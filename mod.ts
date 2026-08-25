// WebUSB (https://wicg.github.io/webusb/) for Deno, backed by the Rust
// `webusb` crate over FFI.
//
// Importing this module installs a `USB` instance as `navigator.usb`.
// Build the native library first: `cargo build --features ffi`.

import {
  cstr,
  errorFromCode,
  lib,
  STATUS_NAMES,
  takeJsonResult,
  takeString,
} from "./ffi.ts";

export type USBTransferStatus = "ok" | "stall" | "babble";
export type USBDirection = "in" | "out";
export type USBEndpointType = "bulk" | "interrupt" | "isochronous" | "control";
export type USBRequestType = "standard" | "class" | "vendor";
export type USBRecipient = "device" | "interface" | "endpoint" | "other";

export interface USBEndpoint {
  endpointNumber: number;
  direction: USBDirection;
  type: USBEndpointType;
  packetSize: number;
}

export interface USBAlternateInterface {
  alternateSetting: number;
  interfaceClass: number;
  interfaceSubclass: number;
  interfaceProtocol: number;
  interfaceName: string | null;
  endpoints: USBEndpoint[];
}

export interface USBInterface {
  interfaceNumber: number;
  alternate: USBAlternateInterface;
  alternates: USBAlternateInterface[];
  claimed: boolean;
}

export interface USBConfiguration {
  configurationName: string | null;
  configurationValue: number;
  interfaces: USBInterface[];
}

export interface USBDeviceFilter {
  vendorId?: number;
  productId?: number;
  classCode?: number;
  subclassCode?: number;
  protocolCode?: number;
  serialNumber?: string;
}

export interface USBDeviceRequestOptions {
  filters: USBDeviceFilter[];
}

export interface USBControlTransferParameters {
  requestType: USBRequestType;
  recipient: USBRecipient;
  request: number;
  value: number;
  index: number;
}

export class USBInTransferResult {
  constructor(
    readonly status: USBTransferStatus,
    readonly data?: DataView,
  ) {}
}

export class USBOutTransferResult {
  constructor(
    readonly status: USBTransferStatus,
    readonly bytesWritten: number = 0,
  ) {}
}

export class USBIsochronousInTransferPacket {
  constructor(
    readonly status: USBTransferStatus,
    readonly data?: DataView,
  ) {}
}

export class USBIsochronousInTransferResult {
  constructor(
    readonly packets: USBIsochronousInTransferPacket[],
    readonly data?: DataView,
  ) {}
}

export class USBIsochronousOutTransferPacket {
  constructor(
    readonly status: USBTransferStatus,
    readonly bytesWritten: number = 0,
  ) {}
}

export class USBIsochronousOutTransferResult {
  constructor(readonly packets: USBIsochronousOutTransferPacket[]) {}
}

interface DeviceSnapshot {
  id: number;
  configurations: USBConfiguration[];
  configuration: USBConfiguration | null;
  deviceClass: number;
  deviceSubclass: number;
  deviceProtocol: number;
  deviceVersionMajor: number;
  deviceVersionMinor: number;
  deviceVersionSubminor: number;
  manufacturerName: string | null;
  productId: number;
  productName: string | null;
  serialNumber: string | null;
  usbVersionMajor: number;
  usbVersionMinor: number;
  usbVersionSubminor: number;
  vendorId: number;
  opened: boolean;
  url: string | null;
}

const UPDATE = Symbol("webusb.update");

const REQUEST_TYPES: Record<USBRequestType, number> = {
  standard: 0,
  class: 1,
  vendor: 2,
};

const RECIPIENTS: Record<USBRecipient, number> = {
  device: 0,
  interface: 1,
  endpoint: 2,
  other: 3,
};

function toBytes(data?: BufferSource): Uint8Array {
  if (data === undefined) return new Uint8Array();
  if (data instanceof Uint8Array) return data;
  if (ArrayBuffer.isView(data)) {
    return new Uint8Array(data.buffer, data.byteOffset, data.byteLength);
  }
  return new Uint8Array(data);
}

function encodeSetup(
  setup: USBControlTransferParameters,
): [number, number, number, number, number] {
  const requestType = REQUEST_TYPES[setup.requestType];
  const recipient = RECIPIENTS[setup.recipient];
  if (requestType === undefined || recipient === undefined) {
    throw new TypeError("webusb: invalid control transfer parameters");
  }
  return [requestType, recipient, setup.request, setup.value, setup.index];
}

function checkCode(code: number) {
  if (code !== 0) throw errorFromCode(code);
}

/** https://wicg.github.io/webusb/#usbdevice */
export class USBDevice {
  #data: DeviceSnapshot;

  /** @internal Constructed by this module; do not instantiate directly. */
  constructor(snapshot: DeviceSnapshot) {
    this.#data = snapshot;
  }

  [UPDATE](snapshot: DeviceSnapshot) {
    this.#data = snapshot;
  }

  get #rid(): bigint {
    return BigInt(this.#data.id);
  }

  /** Process-unique id, stable while the device stays connected. */
  get id(): number {
    return this.#data.id;
  }
  get configurations(): USBConfiguration[] {
    return this.#data.configurations;
  }
  get configuration(): USBConfiguration | null {
    return this.#data.configuration;
  }
  get deviceClass(): number {
    return this.#data.deviceClass;
  }
  get deviceSubclass(): number {
    return this.#data.deviceSubclass;
  }
  get deviceProtocol(): number {
    return this.#data.deviceProtocol;
  }
  get deviceVersionMajor(): number {
    return this.#data.deviceVersionMajor;
  }
  get deviceVersionMinor(): number {
    return this.#data.deviceVersionMinor;
  }
  get deviceVersionSubminor(): number {
    return this.#data.deviceVersionSubminor;
  }
  get manufacturerName(): string | null {
    return this.#data.manufacturerName;
  }
  get productId(): number {
    return this.#data.productId;
  }
  get productName(): string | null {
    return this.#data.productName;
  }
  get serialNumber(): string | null {
    return this.#data.serialNumber;
  }
  get usbVersionMajor(): number {
    return this.#data.usbVersionMajor;
  }
  get usbVersionMinor(): number {
    return this.#data.usbVersionMinor;
  }
  get usbVersionSubminor(): number {
    return this.#data.usbVersionSubminor;
  }
  get vendorId(): number {
    return this.#data.vendorId;
  }
  get opened(): boolean {
    return this.#data.opened;
  }
  /** WEBUSB_URL from the WebUSB platform capability descriptor. */
  get url(): string | null {
    return this.#data.url;
  }

  async open(): Promise<void> {
    checkCode(await lib.symbols.webusb_open(this.#rid));
    this.#data.opened = true;
  }

  async close(): Promise<void> {
    checkCode(await lib.symbols.webusb_close(this.#rid));
    this.#data.opened = false;
    if (this.#data.configuration) {
      for (const iface of this.#data.configuration.interfaces) {
        iface.claimed = false;
      }
    }
  }

  /** There is no permission store outside the browser; this closes the device. */
  async forget(): Promise<void> {
    if (this.#data.opened) {
      await this.close();
    }
  }

  async selectConfiguration(configurationValue: number): Promise<void> {
    checkCode(
      await lib.symbols.webusb_select_configuration(
        this.#rid,
        configurationValue,
      ),
    );
    const configuration = this.#data.configurations.find(
      (c) => c.configurationValue === configurationValue,
    );
    this.#data.configuration = configuration
      ? structuredClone(configuration)
      : null;
  }

  async claimInterface(interfaceNumber: number): Promise<void> {
    checkCode(
      await lib.symbols.webusb_claim_interface(this.#rid, interfaceNumber),
    );
    const iface = this.#data.configuration?.interfaces.find(
      (i) => i.interfaceNumber === interfaceNumber,
    );
    if (iface) iface.claimed = true;
  }

  async releaseInterface(interfaceNumber: number): Promise<void> {
    checkCode(
      await lib.symbols.webusb_release_interface(
        this.#rid,
        interfaceNumber,
      ),
    );
    const iface = this.#data.configuration?.interfaces.find(
      (i) => i.interfaceNumber === interfaceNumber,
    );
    if (iface) iface.claimed = false;
  }

  async selectAlternateInterface(
    interfaceNumber: number,
    alternateSetting: number,
  ): Promise<void> {
    checkCode(
      await lib.symbols.webusb_select_alternate_interface(
        this.#rid,
        interfaceNumber,
        alternateSetting,
      ),
    );
    const iface = this.#data.configuration?.interfaces.find(
      (i) => i.interfaceNumber === interfaceNumber,
    );
    const alternate = iface?.alternates.find(
      (a) => a.alternateSetting === alternateSetting,
    );
    if (iface && alternate) iface.alternate = alternate;
  }

  async controlTransferIn(
    setup: USBControlTransferParameters,
    length: number,
  ): Promise<USBInTransferResult> {
    const [requestType, recipient, request, value, index] = encodeSetup(setup);
    const buffer = new Uint8Array(length);
    const status = new Uint8Array(1);
    const result = Number(
      await lib.symbols.webusb_control_transfer_in(
        this.#rid,
        requestType,
        recipient,
        request,
        value,
        index,
        buffer,
        length,
        status,
      ),
    );
    if (result < 0) throw errorFromCode(result);
    return new USBInTransferResult(
      STATUS_NAMES[status[0]],
      new DataView(buffer.buffer, 0, result),
    );
  }

  async controlTransferOut(
    setup: USBControlTransferParameters,
    data?: BufferSource,
  ): Promise<USBOutTransferResult> {
    const [requestType, recipient, request, value, index] = encodeSetup(setup);
    const bytes = toBytes(data);
    const status = new Uint8Array(1);
    const result = Number(
      await lib.symbols.webusb_control_transfer_out(
        this.#rid,
        requestType,
        recipient,
        request,
        value,
        index,
        bytes,
        bytes.byteLength,
        status,
      ),
    );
    if (result < 0) throw errorFromCode(result);
    return new USBOutTransferResult(STATUS_NAMES[status[0]], result);
  }

  async clearHalt(
    direction: USBDirection,
    endpointNumber: number,
  ): Promise<void> {
    checkCode(
      await lib.symbols.webusb_clear_halt(
        this.#rid,
        direction === "out" ? 0 : 1,
        endpointNumber,
      ),
    );
  }

  async transferIn(
    endpointNumber: number,
    length: number,
  ): Promise<USBInTransferResult> {
    const buffer = new Uint8Array(length);
    const status = new Uint8Array(1);
    const result = Number(
      await lib.symbols.webusb_transfer_in(
        this.#rid,
        endpointNumber,
        buffer,
        length,
        status,
      ),
    );
    if (result < 0) throw errorFromCode(result);
    return new USBInTransferResult(
      STATUS_NAMES[status[0]],
      new DataView(buffer.buffer, 0, result),
    );
  }

  async transferOut(
    endpointNumber: number,
    data: BufferSource,
  ): Promise<USBOutTransferResult> {
    const bytes = toBytes(data);
    const status = new Uint8Array(1);
    const result = Number(
      await lib.symbols.webusb_transfer_out(
        this.#rid,
        endpointNumber,
        bytes,
        bytes.byteLength,
        status,
      ),
    );
    if (result < 0) throw errorFromCode(result);
    return new USBOutTransferResult(STATUS_NAMES[status[0]], result);
  }

  /**
   * Parameters are validated per the specification, but isochronous
   * transfers are not yet supported by the native backend; this rejects
   * with `NotSupportedError` once validation passes.
   */
  async isochronousTransferIn(
    endpointNumber: number,
    _packetLengths: number[],
  ): Promise<USBIsochronousInTransferResult> {
    checkCode(
      await lib.symbols.webusb_isochronous_transfer_in(
        this.#rid,
        endpointNumber,
      ),
    );
    return new USBIsochronousInTransferResult([]);
  }

  /** See {@linkcode USBDevice.isochronousTransferIn}. */
  async isochronousTransferOut(
    endpointNumber: number,
    data: BufferSource,
    _packetLengths: number[],
  ): Promise<USBIsochronousOutTransferResult> {
    const bytes = toBytes(data);
    checkCode(
      await lib.symbols.webusb_isochronous_transfer_out(
        this.#rid,
        endpointNumber,
        bytes,
        bytes.byteLength,
      ),
    );
    return new USBIsochronousOutTransferResult([]);
  }

  async reset(): Promise<void> {
    checkCode(await lib.symbols.webusb_reset(this.#rid));
  }
}

/** https://wicg.github.io/webusb/#usbconnectionevent */
export class USBConnectionEvent extends Event {
  readonly device: USBDevice;

  constructor(type: string, eventInitDict: { device: USBDevice }) {
    super(type);
    this.device = eventInitDict.device;
  }
}

const registry = new Map<number, USBDevice>();

function trackDevice(snapshot: DeviceSnapshot): USBDevice {
  const existing = registry.get(snapshot.id);
  if (existing) {
    existing[UPDATE](snapshot);
    return existing;
  }
  const device = new USBDevice(snapshot);
  registry.set(snapshot.id, device);
  return device;
}

function placeholderDevice(
  id: number,
  vendorId: number,
  productId: number,
): USBDevice {
  return new USBDevice({
    id,
    configurations: [],
    configuration: null,
    deviceClass: 0,
    deviceSubclass: 0,
    deviceProtocol: 0,
    deviceVersionMajor: 0,
    deviceVersionMinor: 0,
    deviceVersionSubminor: 0,
    manufacturerName: null,
    productId,
    productName: null,
    serialNumber: null,
    usbVersionMajor: 0,
    usbVersionMinor: 0,
    usbVersionSubminor: 0,
    vendorId,
    opened: false,
    url: null,
  });
}

type ConnectionListener =
  | EventListenerOrEventListenerObject
  | null
  | undefined;

/** https://wicg.github.io/webusb/#usb */
export class USB extends EventTarget {
  #connectListeners = new Set<EventListenerOrEventListenerObject>();
  #disconnectListeners = new Set<EventListenerOrEventListenerObject>();
  #onconnect: ConnectionListener = null;
  #ondisconnect: ConnectionListener = null;
  #pumping = false;

  async getDevices(): Promise<USBDevice[]> {
    const snapshots = takeJsonResult(
      await lib.symbols.webusb_get_devices(),
    ) as DeviceSnapshot[];
    return snapshots.map(trackDevice);
  }

  async requestDevice(options: USBDeviceRequestOptions): Promise<USBDevice> {
    if (!options || !Array.isArray(options.filters)) {
      throw new TypeError(
        "webusb: requestDevice requires an options object with filters",
      );
    }
    const snapshot = takeJsonResult(
      await lib.symbols.webusb_request_device(
        cstr(JSON.stringify(options.filters)),
      ),
    ) as DeviceSnapshot;
    return trackDevice(snapshot);
  }

  get onconnect(): ConnectionListener {
    return this.#onconnect;
  }
  set onconnect(listener: ConnectionListener) {
    if (this.#onconnect) this.removeEventListener("connect", this.#onconnect);
    this.#onconnect = listener ?? null;
    if (listener) this.addEventListener("connect", listener);
  }

  get ondisconnect(): ConnectionListener {
    return this.#ondisconnect;
  }
  set ondisconnect(listener: ConnectionListener) {
    if (this.#ondisconnect) {
      this.removeEventListener("disconnect", this.#ondisconnect);
    }
    this.#ondisconnect = listener ?? null;
    if (listener) this.addEventListener("disconnect", listener);
  }

  override addEventListener(
    type: string,
    listener: EventListenerOrEventListenerObject | null,
    options?: boolean | AddEventListenerOptions,
  ): void {
    super.addEventListener(type, listener, options);
    if (listener && (type === "connect" || type === "disconnect")) {
      this.#listeners(type).add(listener);
      this.#syncPump();
    }
  }

  override removeEventListener(
    type: string,
    listener: EventListenerOrEventListenerObject | null,
    options?: boolean | EventListenerOptions,
  ): void {
    super.removeEventListener(type, listener, options);
    if (listener && (type === "connect" || type === "disconnect")) {
      this.#listeners(type).delete(listener);
      this.#syncPump();
    }
  }

  #listeners(
    type: "connect" | "disconnect",
  ): Set<EventListenerOrEventListenerObject> {
    return type === "connect"
      ? this.#connectListeners
      : this.#disconnectListeners;
  }

  #syncPump() {
    const wanted = this.#connectListeners.size > 0 ||
      this.#disconnectListeners.size > 0;
    if (wanted && !this.#pumping) {
      this.#pumping = true;
      this.#pump();
    } else if (!wanted && this.#pumping) {
      lib.symbols.webusb_events_stop();
    }
  }

  async #pump(): Promise<void> {
    checkCode(lib.symbols.webusb_events_start());
    try {
      while (true) {
        const json = takeString(await lib.symbols.webusb_next_event());
        if (json === null) break;
        const event = JSON.parse(json) as {
          event: "connect" | "disconnect";
          device?: DeviceSnapshot;
          id?: number;
          vendorId?: number;
          productId?: number;
        };
        if (event.event === "connect" && event.device) {
          const device = trackDevice(event.device);
          this.dispatchEvent(new USBConnectionEvent("connect", { device }));
        } else if (event.event === "disconnect" && event.id !== undefined) {
          const device = registry.get(event.id) ??
            placeholderDevice(
              event.id,
              event.vendorId ?? 0,
              event.productId ?? 0,
            );
          registry.delete(event.id);
          this.dispatchEvent(new USBConnectionEvent("disconnect", { device }));
        }
      }
    } finally {
      this.#pumping = false;
    }
  }
}

const usb = new USB();

declare global {
  interface Navigator {
    usb: USB;
  }
}

Object.defineProperty(navigator, "usb", {
  value: usb,
  configurable: true,
  enumerable: true,
});

export { usb };
export default usb;
