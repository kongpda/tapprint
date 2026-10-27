export interface TapPrintOptions {
  /** Port TapPrint listens on. Default 9240. */
  port?: number;
  /** A manual pairing code. Skips browser pairing. */
  code?: string;
  /** localStorage key for the site code. Default "tapprint.code". */
  storageKey?: string;
  /** Name shown in the TapPrint window. */
  clientName?: string;
}

export interface PrintOptions {
  /** Paper width. Default: the width set in the TapPrint window. */
  paperMm?: 58 | 80;
  /** Cut after printing. Default true. */
  cut?: boolean;
  /** Kick the cash drawer after printing. Default false. */
  drawer?: boolean;
  /** Floyd-Steinberg dithering for photos. Default false. */
  dither?: boolean;
}

export interface TapPrintStatus {
  ok: boolean;
  paired: boolean;
  port: number;
  paper_mm: number;
  transport: "usb" | "tcp";
  state: "ready" | "busy" | "problem";
  raw_enabled: boolean;
  /** Only when paired. */
  printers?: string[];
  /** Only when paired. */
  status?: string;
}

export class TapPrintError extends Error {
  constructor(message: string, status: number, reason: string);
  /** HTTP status, or 0 when TapPrint could not be reached. */
  status: number;
  /** API reason code, or "unreachable" / "bad_response". */
  reason: string;
}

export class TapPrint {
  constructor(options?: TapPrintOptions);
  status(): Promise<TapPrintStatus>;
  pair(): Promise<void>;
  isPaired(): boolean;
  forget(): void;
  print(source: Blob | HTMLCanvasElement | OffscreenCanvas, options?: PrintOptions): Promise<void>;
  printRaw(bytes: Uint8Array): Promise<void>;
}
