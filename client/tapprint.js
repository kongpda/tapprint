// TapPrint browser client. No dependencies. MIT licence.

/**
 * Error thrown for every failed TapPrint call.
 * `status` is the HTTP status (0 when TapPrint could not be reached).
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
  }
}

/**
 * @typedef {Object} TapPrintOptions
 * @property {number} [port] Port TapPrint listens on. Default 9240.
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
  constructor({ port = 9240, code, storageKey = "tapprint.code", clientName } = {}) {
    this.base = `http://127.0.0.1:${port}`;
    this.storageKey = storageKey;
    this.clientName = clientName;
    /** @type {string|undefined} */
    this.manualCode = code;
    /** @type {string|undefined} */
    this.givenCode = code;
  }

  /** @returns {string|null} */
  _code() {
    if (this.manualCode) return this.manualCode;
    try {
      return globalThis.localStorage?.getItem(this.storageKey) ?? null;
    } catch {
      return null;
    }
  }

  /** @param {string} value */
  _store(value) {
    try {
      globalThis.localStorage?.setItem(this.storageKey, value);
    } catch {
      // Storage may be blocked. The code then lasts for this object only.
    }
    this.manualCode = value;
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
    // A code given in the options belongs to the caller. Only a stored one is forgotten.
    const stored = Boolean(code) && code !== this.givenCode;
    if (code) headers.Authorization = `Bearer ${code}`;

    let response;
    try {
      response = await fetch(this.base + path, { ...init, headers });
    } catch (cause) {
      throw new TapPrintError(
        "Cannot reach TapPrint. Check that it is running on this computer.",
        0,
        "unreachable",
      );
    }
    let body;
    try {
      body = await response.json();
    } catch {
      throw new TapPrintError("TapPrint sent an unreadable reply.", response.status, "bad_response");
    }
    if (!response.ok || body.ok === false) {
      if (response.status === 401 && stored) this.forget();
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

  /** @returns {boolean} True when a code is stored. It may still be revoked. */
  isPaired() {
    return Boolean(this._code());
  }

  /** Forget the stored code. */
  forget() {
    this.manualCode = undefined;
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
   * "Allow raw printer commands" in the TapPrint window, else reason is "raw_disabled".
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
