// Deno FFI bindings for the `webusb` cdylib (built with `--features ffi`).
//
// Blocking operations are declared `nonblocking` so they run on Deno's FFI
// thread pool and surface as Promises.
//
// The native library is resolved in this order:
// 1. The `WEBUSB_LIBRARY` environment variable (explicit path).
// 2. `target/debug` / `target/release` next to this module (git checkouts).
// 3. Downloaded from the matching GitHub release and cached by
//    `@denosaurs/plug` (published package).

import { dlopen as plugDlopen } from "@denosaurs/plug";
import { VERSION } from "./version.ts";

const SYMBOLS = {
  webusb_init: { parameters: ["u8"], result: "i32" },
  webusb_free_string: { parameters: ["pointer"], result: "void" },
  webusb_get_devices: {
    parameters: [],
    result: "pointer",
    nonblocking: true,
  },
  webusb_request_device: {
    parameters: ["buffer"],
    result: "pointer",
    nonblocking: true,
  },
  webusb_open: { parameters: ["u64"], result: "i32", nonblocking: true },
  webusb_close: { parameters: ["u64"], result: "i32", nonblocking: true },
  webusb_reset: { parameters: ["u64"], result: "i32", nonblocking: true },
  webusb_select_configuration: {
    parameters: ["u64", "u8"],
    result: "i32",
    nonblocking: true,
  },
  webusb_claim_interface: {
    parameters: ["u64", "u8"],
    result: "i32",
    nonblocking: true,
  },
  webusb_release_interface: {
    parameters: ["u64", "u8"],
    result: "i32",
    nonblocking: true,
  },
  webusb_select_alternate_interface: {
    parameters: ["u64", "u8", "u8"],
    result: "i32",
    nonblocking: true,
  },
  webusb_clear_halt: {
    parameters: ["u64", "u8", "u8"],
    result: "i32",
    nonblocking: true,
  },
  webusb_transfer_in: {
    parameters: ["u64", "u8", "buffer", "u32", "buffer"],
    result: "i64",
    nonblocking: true,
  },
  webusb_transfer_out: {
    parameters: ["u64", "u8", "buffer", "u32", "buffer"],
    result: "i64",
    nonblocking: true,
  },
  webusb_control_transfer_in: {
    parameters: [
      "u64",
      "u8",
      "u8",
      "u8",
      "u16",
      "u16",
      "buffer",
      "u16",
      "buffer",
    ],
    result: "i64",
    nonblocking: true,
  },
  webusb_control_transfer_out: {
    parameters: [
      "u64",
      "u8",
      "u8",
      "u8",
      "u16",
      "u16",
      "buffer",
      "u32",
      "buffer",
    ],
    result: "i64",
    nonblocking: true,
  },
  webusb_isochronous_transfer_in: {
    parameters: ["u64", "u8"],
    result: "i32",
    nonblocking: true,
  },
  webusb_isochronous_transfer_out: {
    parameters: ["u64", "u8", "buffer", "u32"],
    result: "i32",
    nonblocking: true,
  },
  webusb_events_start: { parameters: [], result: "i32" },
  webusb_next_event: { parameters: [], result: "pointer", nonblocking: true },
  webusb_events_stop: { parameters: [], result: "i32" },
  webusb_mock_add_device: { parameters: ["buffer"], result: "i64" },
  webusb_mock_remove_device: { parameters: ["u64"], result: "i32" },
  webusb_mock_written: { parameters: ["u64"], result: "pointer" },
  webusb_mock_halt_endpoint: { parameters: ["u64", "u8"], result: "i32" },
} as const satisfies Deno.ForeignLibraryInterface;

function envVar(name: string): string | undefined {
  try {
    return Deno.env.get(name);
  } catch {
    return undefined;
  }
}

function libraryFilename(): string {
  switch (Deno.build.os) {
    case "windows":
      return "webusb.dll";
    case "darwin":
      return "libwebusb.dylib";
    default:
      return "libwebusb.so";
  }
}

const RELEASE_BASE =
  `https://github.com/elevationai/webusb/releases/download/v${VERSION}`;

async function openLibrary(): Promise<Deno.DynamicLibrary<typeof SYMBOLS>> {
  // 1. Explicit override.
  const override = envVar("WEBUSB_LIBRARY");
  if (override) {
    return Deno.dlopen(override, SYMBOLS);
  }

  // 2. Local cargo build, when running from a checkout.
  if (import.meta.url.startsWith("file:")) {
    const filename = libraryFilename();
    for (const dir of ["debug", "release"]) {
      try {
        return Deno.dlopen(
          new URL(`./target/${dir}/${filename}`, import.meta.url),
          SYMBOLS,
        );
      } catch {
        // Try the next candidate.
      }
    }
  }

  // 3. Prebuilt library from the GitHub release, cached by plug.
  try {
    return await plugDlopen({
      name: "webusb",
      url: {
        darwin: {
          aarch64: `${RELEASE_BASE}/libwebusb_aarch64.dylib`,
          x86_64: `${RELEASE_BASE}/libwebusb_x86_64.dylib`,
        },
        linux: {
          aarch64: `${RELEASE_BASE}/libwebusb_aarch64.so`,
          x86_64: `${RELEASE_BASE}/libwebusb_x86_64.so`,
        },
        windows: `${RELEASE_BASE}/webusb_x86_64.dll`,
      },
    }, SYMBOLS);
  } catch (error) {
    throw new Error(
      `Could not load the webusb native library for ` +
        `${Deno.build.os}/${Deno.build.arch} (v${VERSION}). Set ` +
        `WEBUSB_LIBRARY to a local build (cargo build --features ffi), or ` +
        `ensure ${RELEASE_BASE} is reachable. Cause: ${error}`,
    );
  }
}

export const lib: Deno.DynamicLibrary<typeof SYMBOLS> = await openLibrary();

const useMock = envVar("WEBUSB_BACKEND") === "mock";
export const isMock = useMock;

const initResult = lib.symbols.webusb_init(useMock ? 1 : 0);
if (initResult !== 0) {
  throw new Error(`webusb_init failed with code ${initResult}`);
}

const encoder = new TextEncoder();

/** Encode a string as a NUL-terminated buffer for FFI input. */
export function cstr(value: string): Uint8Array {
  return encoder.encode(value + "\0");
}

/** Read and free a string returned by the native library. */
export function takeString(ptr: Deno.PointerValue): string | null {
  if (ptr === null) return null;
  const value = new Deno.UnsafePointerView(ptr).getCString();
  lib.symbols.webusb_free_string(ptr);
  return value;
}

/** Read and free a `{"ok": ...} | {"err": code}` JSON response. */
export function takeJsonResult(ptr: Deno.PointerValue): unknown {
  const json = takeString(ptr);
  if (json === null) {
    throw new Error("webusb: native library returned no data");
  }
  const parsed = JSON.parse(json) as { ok?: unknown; err?: number };
  if (parsed.err !== undefined) {
    throw errorFromCode(parsed.err);
  }
  return parsed.ok;
}

const ERROR_NAMES: Record<number, string> = {
  [-1]: "NotFoundError",
  [-2]: "InvalidStateError",
  [-3]: "InvalidAccessError",
  [-4]: "NotSupportedError",
  [-5]: "NetworkError",
  [-6]: "NetworkError", // busy
  [-7]: "SecurityError",
  [-8]: "NetworkError", // io
  [-9]: "InvalidStateError", // not initialized
};

/** Map a native error code to the DOMException the WebUSB spec prescribes. */
export function errorFromCode(code: number): Error {
  if (code === -10) {
    return new TypeError("webusb: invalid argument");
  }
  const name = ERROR_NAMES[code];
  if (name !== undefined) {
    return new DOMException(`webusb: ${name} (code ${code})`, name);
  }
  return new Error(`webusb: unknown native error (code ${code})`);
}

export const STATUS_NAMES = ["ok", "stall", "babble"] as const;
