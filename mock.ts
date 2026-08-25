// Helpers for driving the in-memory mock backend from Deno.
//
// Only usable when the module was initialized with `WEBUSB_BACKEND=mock`.
// Mirrors `MockDeviceConfig` on the Rust side.

import { cstr, errorFromCode, isMock, lib, takeJsonResult } from "./ffi.ts";
import type { USBConfiguration } from "./mod.ts";

export interface MockDeviceConfig {
  vendorId?: number;
  productId?: number;
  deviceClass?: number;
  deviceSubclass?: number;
  deviceProtocol?: number;
  deviceVersionMajor?: number;
  deviceVersionMinor?: number;
  deviceVersionSubminor?: number;
  usbVersionMajor?: number;
  usbVersionMinor?: number;
  usbVersionSubminor?: number;
  manufacturerName?: string;
  productName?: string;
  serialNumber?: string;
  configurations?: USBConfiguration[];
  activeConfiguration?: number;
  url?: string;
  /** Endpoint number -> canned payload returned by IN transfers. */
  inData?: Record<string, number[]>;
  /** Endpoint addresses (`0x80 | n` for IN) that start out halted. */
  stalledEndpoints?: number[];
  /** Endpoint addresses whose transfers report `babble`. */
  babbleEndpoints?: number[];
}

function assertMock() {
  if (!isMock) {
    throw new Error(
      "webusb: mock helpers require initializing with WEBUSB_BACKEND=mock",
    );
  }
}

/** Connect a mock device, firing a connect event. Returns the device id. */
export function addMockDevice(config: MockDeviceConfig): number {
  assertMock();
  const id = Number(
    lib.symbols.webusb_mock_add_device(cstr(JSON.stringify(config))),
  );
  if (id < 0) throw errorFromCode(id);
  return id;
}

/** Disconnect a mock device, firing a disconnect event. */
export function removeMockDevice(id: number): void {
  assertMock();
  const code = lib.symbols.webusb_mock_remove_device(BigInt(id));
  if (code !== 0) throw errorFromCode(code);
}

/**
 * Everything written to a mock device so far, as `[endpoint, bytes]` pairs.
 * Control transfers are recorded as endpoint 0.
 */
export function mockWritten(id: number): [number, number[]][] {
  assertMock();
  return takeJsonResult(lib.symbols.webusb_mock_written(BigInt(id))) as [
    number,
    number[],
  ][];
}

/** Halt a mock endpoint so transfers report `stall` until cleared. */
export function mockHaltEndpoint(id: number, address: number): void {
  assertMock();
  const code = lib.symbols.webusb_mock_halt_endpoint(BigInt(id), address);
  if (code !== 0) throw errorFromCode(code);
}
