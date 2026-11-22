// TapPrint browser client. No dependencies. MIT licence.

/**
 * Error thrown for every failed TapPrint call.
 * `status` is the HTTP status (0 when TapPrint could not be reached).
 * `detail` says why TapPrint was unreachable, when the browser tells:
 * "insecure_page" (the page is not on HTTPS), "blocked_by_csp" (the site's
 * Content-Security-Policy forbids the call), or "permission_denied" (the
 * person blocked the site from reaching apps on this device).
 * `reason` is the machine code from the API, such as "unauthorized",
 * "paper_out" or "maybe_printed", or "unreachable" / "bad_response".
 */
export class TapPrintError extends Error {
  /**
   * @param {string} message
   * @param {number} status
   * @param {string} reason
   */
  constructor(message, status, reason) {
    super(message);
    this.name = "TapPrintError";
    /** @type {number} */
    this.status = status;
    /** @type {string} */
    this.reason = reason;
    /** @type {"insecure_page"|"blocked_by_csp"|"permission_denied"|undefined} */
    this.detail = undefined;
  }
}

/**
 * @typedef {Object} TapPrintOptions
 * @property {number} [port] Port TapPrint listens on. Default 9240.
 * @property {boolean|"auto"} [secure] `false`: plain HTTP on `port`. `true`:
 *   HTTPS on `port + 1`, which TapPrint serves on macOS for Safari. `"auto"`
 *   (default): use whichever answers, plain HTTP first.
 * @property {string} [code] A manual pairing code. Skips browser pairing.
 * @property {string} [storageKey] localStorage key for the site code.
 * @property {string} [clientName] Name shown in the TapPrint window.
 */

/**
 * @typedef {Object} PrintOptions
 * @property {58|80} [paperMm] Paper width. Default: the width set in the window.
 * @property {boolean} [cut] Cut after printing. Default true.
 * @property {boolean} [drawer] Kick the cash drawer after printing. Default false.
 * @property {boolean} [dither] Floyd-Steinberg dithering, for photos. Default false.
 */

/**
 * @typedef {Object} TapPrintStatus
 * @property {boolean} ok
 * @property {boolean} paired
 * @property {number} port
 * @property {number} paper_mm
 * @property {"usb"|"tcp"} transport
 * @property {"ready"|"busy"|"problem"} state
 * @property {boolean} raw_enabled
 * @property {string[]} [printers] Only when paired.
 * @property {string} [status] Only when paired.
 */

export class TapPrint {
  /**
   * @param {TapPrintOptions} [options]
   */
  constructor({ port = 9240, secure = "auto", code, storageKey = "tapprint.code", clientName } = {}) {
    /** Where calls go. With `secure: "auto"` it is settled by the first call. */
    this.base = secure === true ? `https://127.0.0.1:${port + 1}` : `http://127.0.0.1:${port}`;
    /** Both addresses to try, or null when the caller fixed one. @type {string[]|null} */
    this.candidates =
      secure === "auto" ? [`http://127.0.0.1:${port}`, `https://127.0.0.1:${port + 1}`] : null;
    /** True once `base` is known to answer. */
    this.settled = this.candidates === null;
    /** A probe in flight, shared by calls made at the same time. @type {Promise<string>|null} */
    this.probing = null;
    /** Set when the site's own Content-Security-Policy refused a call to TapPrint. */
    this.cspBlocked = false;
    try {
      globalThis.addEventListener?.("securitypolicyviolation", (event) => {
        if (/^https?:\/\/127\.0\.0\.1(:|\/|$)/.test(event.blockedURI || "")) this.cspBlocked = true;
      });
    } catch {
      // No such event here (a worker or a test). The plain message is used.
    }
    this.storageKey = storageKey;
    // A header holds only Latin-1, and TapPrint shows only these characters.
    // Anything else (a name in Khmer, say) would make every call fail.
    this.clientName = String(clientName ?? "")
      .replace(/[^A-Za-z0-9 .-]/g, "")
      .trim()
      .slice(0, 40) || undefined;
    /** A code passed in the options. It belongs to the caller and is never forgotten. */
    this.givenCode = code;
    /** Used only when localStorage is blocked. @type {string|undefined} */
    this.memoryCode = undefined;
  }

  /** @returns {string|null} */
  _code() {
    if (this.givenCode) return this.givenCode;
    // Read storage every time: another tab may have paired since.
    try {
      const stored = globalThis.localStorage?.getItem(this.storageKey);
      if (stored) return stored;
    } catch {
      // Storage is blocked. Fall back to the copy in memory.
    }
    return this.memoryCode ?? null;
  }

  /** @param {string} value */
  _store(value) {
    this.memoryCode = value;
    try {
      globalThis.localStorage?.setItem(this.storageKey, value);
    } catch {
      // Storage may be blocked. The code then lasts for this object only.
    }
  }

  /**
   * Forget `code` after the server rejected it, unless another tab has
   * already stored a newer one.
   * @param {string} code
   */
  _discard(code) {
    if (this.memoryCode === code) this.memoryCode = undefined;
    try {
      if (globalThis.localStorage?.getItem(this.storageKey) === code) {
        globalThis.localStorage.removeItem(this.storageKey);
      }
    } catch {
      // Nothing to remove.
    }
  }

  /**
   * Find the address that answers. Safari refuses plain HTTP from an HTTPS
   * page, so there the HTTPS port is the one that works. Only a status read
   * is ever tried on both addresses: a print or a pairing request goes to one
   * address once, so it can never be sent twice.
   * @returns {Promise<string>}
   */
  _base() {
    if (this.settled) return Promise.resolve(this.base);
    this.probing ??= (async () => {
      try {
        for (const base of this.candidates ?? []) {
          try {
            // Any reply at all means the address can be reached.
            await fetch(`${base}/v1/status`, { method: "GET" });
            this.base = base;
            this.settled = true;
            // A policy that blocked the other address does not block this one.
            this.cspBlocked = false;
            return base;
          } catch {
            // Not reachable this way. Try the next one.
          }
        }
        throw await this._unreachable();
      } finally {
        this.probing = null;
      }
    })();
    return this.probing;
  }

  /**
   * The error for "no answer", with the cause when the browser reveals it.
   * A blocked call and a stopped TapPrint look the same to `fetch`, so the
   * three causes a site owner can fix are checked for by name.
   * @param {unknown} [cause]
   * @returns {Promise<TapPrintError>}
   */
  async _unreachable(cause) {
    // The policy event arrives just after the failed call.
    await new Promise((resolve) => setTimeout(resolve, 0));
    /** @type {TapPrintError["detail"]} */
    let detail;
    let message =
      "Cannot reach TapPrint. Check that it is running on this computer, and that the browser is allowed to reach apps on this device. In Safari on a Mac, open Settings in TapPrint and set up Safari.";
    if (this.cspBlocked) {
      detail = "blocked_by_csp";
      message = `This site's Content-Security-Policy blocks TapPrint. Add ${(this.candidates ?? [this.base]).join(" and ")} to connect-src.`;
    } else if (globalThis.isSecureContext === false) {
      detail = "insecure_page";
      message = "This page is not on HTTPS, so the browser will not let it reach TapPrint. Serve the page over HTTPS.";
    } else if (await loopbackDenied()) {
      detail = "permission_denied";
      message =
        "The browser has blocked this site from reaching apps on this device. Allow it in the site settings (the icon left of the address), then reload.";
    }
    const error = new TapPrintError(message, 0, "unreachable");
    error.detail = detail;
    if (cause !== undefined) error.cause = cause;
    return error;
  }

  /**
   * @param {string} path
   * @param {RequestInit & {headers?: Record<string,string>}} init
   * @param {boolean} sendCode
   * @returns {Promise<any>}
   */
  async _request(path, init, sendCode) {
    const headers = { ...(init.headers || {}) };
    if (this.clientName) headers["X-Till-Client"] = this.clientName;
    const code = sendCode ? this._code() : null;
    if (code) headers.Authorization = `Bearer ${code}`;

    const base = await this._base();
    let response;
    try {
      response = await fetch(base + path, { ...init, headers });
    } catch (cause) {
      // TapPrint may have been restarted or set up differently. Look again next time.
      if (this.candidates) this.settled = false;
      throw await this._unreachable(cause);
    }
    let body;
    try {
      body = await response.json();
    } catch {
      throw new TapPrintError("TapPrint sent an unreadable reply.", response.status, "bad_response");
    }
    if (body === null || typeof body !== "object") {
      throw new TapPrintError("TapPrint sent an unreadable reply.", response.status, "bad_response");
    }
    if (!response.ok || body.ok === false) {
      if (response.status === 401 && code && code !== this.givenCode) this._discard(code);
      throw new TapPrintError(
        body.error || `TapPrint returned ${response.status}.`,
        response.status,
        body.reason || "printer_error",
      );
    }
    return body;
  }

  /**
   * Read printer state. Printer names and the status line are only
   * included when this client holds a valid code.
   * @returns {Promise<TapPrintStatus>}
   */
  status() {
    return this._request("/v1/status", { method: "GET" }, true);
  }

  /**
   * Ask the person at the till to allow this site. Resolves when they click
   * Allow in the TapPrint window (waits up to 120 s). Stores the code.
   * @returns {Promise<void>}
   */
  async pair() {
    const body = await this._request("/v1/pair", { method: "POST" }, false);
    this._store(body.code);
  }

  /**
   * Give up this site's pairing. TapPrint removes the site from its window
   * and the stored code is forgotten. Does nothing when no code is stored.
   * A manual code passed in the options is not a site's code, so TapPrint
   * answers "not_paired" for it.
   * @returns {Promise<void>}
   */
  async unpair() {
    const code = this._code();
    if (!code) return;
    await this._request("/v1/unpair", { method: "POST" }, true);
    // Only the code that was just removed. Another tab may have paired since.
    if (code !== this.givenCode) this._discard(code);
  }

  /** @returns {boolean} True when a code is stored. It may still be revoked. */
  isPaired() {
    return Boolean(this._code());
  }

  /** Forget the stored code. A code passed in the options is kept. */
  forget() {
    this.memoryCode = undefined;
    try {
      globalThis.localStorage?.removeItem(this.storageKey);
    } catch {
      // Nothing to remove.
    }
  }

  /**
   * Print a receipt image.
   * Render at 512 px wide (80 mm) or 360 px wide (58 mm), or a whole multiple.
   * @param {Blob|HTMLCanvasElement|OffscreenCanvas} source
   * @param {PrintOptions} [options]
   * @returns {Promise<void>}
   */
  async print(source, { paperMm, cut = true, drawer = false, dither = false } = {}) {
    const blob = await toPngBlob(source);
    await this._send(blob, "image/png", { paperMm, cut, drawer, dither });
  }

  /**
   * Send raw ESC/POS bytes unchanged. The person at the till must turn on
   * "Raw printer commands" in the TapPrint window, else reason is "raw_disabled".
   * The bytes go out exactly as given: add your own cut or drawer commands.
   * @param {Uint8Array} bytes
   * @returns {Promise<void>}
   */
  async printRaw(bytes) {
    await this._send(bytes, "application/vnd.escpos", {});
  }

  /**
   * @param {BodyInit} body
   * @param {string} type
   * @param {PrintOptions} options
   */
  async _send(body, type, { paperMm, cut, drawer, dither }) {
    /** @type {Record<string,string>} */
    const headers = { "Content-Type": type };
    if (paperMm) headers["X-Paper-Mm"] = String(paperMm);
    if (cut === false) headers["X-Cut"] = "0";
    if (drawer) headers["X-Drawer"] = "1";
    if (dither) headers["X-Dither"] = "1";
    await this._request("/v1/print", { method: "POST", headers, body }, true);
  }
}

/**
 * Did the person block this site from reaching apps on this device? Chrome
 * and Edge can say. The permission was renamed once, so both names are tried.
 * @returns {Promise<boolean>}
 */
async function loopbackDenied() {
  for (const name of ["loopback-network", "local-network-access"]) {
    try {
      // @ts-ignore These names are newer than the DOM typings.
      const status = await globalThis.navigator?.permissions?.query({ name });
      if (status) return status.state === "denied";
    } catch {
      // This browser does not know the name. Try the next one.
    }
  }
  return false;
}

/**
 * @param {Blob|HTMLCanvasElement|OffscreenCanvas} source
 * @returns {Promise<Blob>}
 */
async function toPngBlob(source) {
  if (typeof Blob !== "undefined" && source instanceof Blob) return source;
  if (typeof source.convertToBlob === "function") {
    return source.convertToBlob({ type: "image/png" });
  }
  if (typeof source.toBlob === "function") {
    return new Promise((resolve, reject) => {
      source.toBlob(
        (blob) => (blob ? resolve(blob) : reject(new TapPrintError("Canvas gave no image.", 0, "bad_request"))),
        "image/png",
      );
    });
  }
  throw new TapPrintError("print() needs a Blob or a canvas.", 0, "bad_request");
}
