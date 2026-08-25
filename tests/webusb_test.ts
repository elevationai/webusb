// End-to-end tests for the Deno bindings, running against the mock backend.
// Run with: deno task test

import usb, { USB, USBConnectionEvent, USBDevice } from "../mod.ts";
import {
  addMockDevice,
  mockHaltEndpoint,
  mockWritten,
  removeMockDevice,
} from "../mock.ts";
import type { MockDeviceConfig } from "../mock.ts";

declare global {
  interface Navigator {
    usb: USB;
  }
}

const EP_OUT = 4;
const EP_IN = 5;
const EP_ISO_IN = 6;
const INTERFACE = 2;

function assert(condition: boolean, message: string) {
  if (!condition) throw new Error(`assertion failed: ${message}`);
}

function assertEquals<T>(actual: T, expected: T, message?: string) {
  const a = JSON.stringify(actual);
  const b = JSON.stringify(expected);
  if (a !== b) {
    throw new Error(`${message ?? "assertEquals"}: ${a} !== ${b}`);
  }
}

function testConfig(): MockDeviceConfig {
  const alt0 = {
    alternateSetting: 0,
    interfaceClass: 0xFF,
    interfaceSubclass: 0x01,
    interfaceProtocol: 0x02,
    interfaceName: "test interface",
    endpoints: [
      {
        endpointNumber: EP_OUT,
        direction: "out" as const,
        type: "bulk" as const,
        packetSize: 64,
      },
      {
        endpointNumber: EP_IN,
        direction: "in" as const,
        type: "bulk" as const,
        packetSize: 64,
      },
      {
        endpointNumber: EP_ISO_IN,
        direction: "in" as const,
        type: "isochronous" as const,
        packetSize: 1023,
      },
    ],
  };
  return {
    vendorId: 0x2341,
    productId: 0x8036,
    manufacturerName: "Arduino LLC",
    productName: "Arduino Leonardo",
    serialNumber: "TEST123",
    configurations: [
      {
        configurationName: "Default",
        configurationValue: 1,
        interfaces: [
          {
            interfaceNumber: INTERFACE,
            alternate: alt0,
            alternates: [alt0],
            claimed: false,
          },
        ],
      },
    ],
    activeConfiguration: 1,
    url: "https://example.com/device",
    inData: { [EP_IN]: [...new TextEncoder().encode("hello from device")] },
  };
}

async function findDevice(id: number): Promise<USBDevice> {
  const devices = await usb.getDevices();
  const device = devices.find((d) => d.id === id);
  assert(device !== undefined, `device ${id} enumerated`);
  return device!;
}

Deno.test("navigator.usb is installed", () => {
  assert(navigator.usb === usb, "navigator.usb === usb");
});

Deno.test("device metadata", async () => {
  const id = addMockDevice(testConfig());
  try {
    const device = await findDevice(id);
    assertEquals(device.vendorId, 0x2341);
    assertEquals(device.productId, 0x8036);
    assertEquals(device.productName, "Arduino Leonardo");
    assertEquals(device.serialNumber, "TEST123");
    assertEquals(device.url, "https://example.com/device");
    assertEquals(device.opened, false);
    assertEquals(device.configuration?.configurationValue, 1);
    assertEquals(device.configurations.length, 1);
  } finally {
    removeMockDevice(id);
  }
});

Deno.test("requestDevice with filters", async () => {
  const id = addMockDevice(testConfig());
  try {
    const device = await usb.requestDevice({
      filters: [{ vendorId: 0x2341, productId: 0x8036 }],
    });
    assertEquals(device.serialNumber, "TEST123");

    // No match rejects with NotFoundError.
    let name = "";
    try {
      await usb.requestDevice({ filters: [{ vendorId: 0xDEAD }] });
    } catch (error) {
      name = (error as DOMException).name;
    }
    assertEquals(name, "NotFoundError");

    // Missing filters is a TypeError.
    let isTypeError = false;
    try {
      await usb.requestDevice(undefined as unknown as { filters: [] });
    } catch (error) {
      isTypeError = error instanceof TypeError;
    }
    assert(isTypeError, "requestDevice() without filters throws TypeError");
  } finally {
    removeMockDevice(id);
  }
});

Deno.test("open, claim, transfer roundtrip", async () => {
  const id = addMockDevice(testConfig());
  try {
    const device = await findDevice(id);
    await device.open();
    assert(device.opened, "device.opened after open()");
    await device.claimInterface(INTERFACE);
    assert(
      device.configuration!.interfaces[0].claimed,
      "interface claimed",
    );

    const out = await device.transferOut(
      EP_OUT,
      new TextEncoder().encode("LED ON"),
    );
    assertEquals(out.status, "ok");
    assertEquals(out.bytesWritten, 6);
    assertEquals(mockWritten(id), [[
      EP_OUT,
      [...new TextEncoder().encode("LED ON")],
    ]]);

    const result = await device.transferIn(EP_IN, 64);
    assertEquals(result.status, "ok");
    const text = new TextDecoder().decode(result.data!);
    assertEquals(text, "hello from device");

    await device.close();
    assert(!device.opened, "device closed");
  } finally {
    removeMockDevice(id);
  }
});

Deno.test("transfers require open and claimed interface", async () => {
  const id = addMockDevice(testConfig());
  try {
    const device = await findDevice(id);

    let name = "";
    try {
      await device.transferOut(EP_OUT, new Uint8Array([1]));
    } catch (error) {
      name = (error as DOMException).name;
    }
    assertEquals(name, "InvalidStateError", "transfer before open");

    await device.open();
    name = "";
    try {
      await device.transferIn(EP_IN, 8);
    } catch (error) {
      name = (error as DOMException).name;
    }
    assertEquals(name, "InvalidStateError", "transfer before claim");
    await device.close();
  } finally {
    removeMockDevice(id);
  }
});

Deno.test("stall and clearHalt", async () => {
  const id = addMockDevice(testConfig());
  try {
    const device = await findDevice(id);
    await device.open();
    await device.claimInterface(INTERFACE);

    mockHaltEndpoint(id, 0x80 | EP_IN);
    const stalled = await device.transferIn(EP_IN, 64);
    assertEquals(stalled.status, "stall");

    await device.clearHalt("in", EP_IN);
    const result = await device.transferIn(EP_IN, 64);
    assertEquals(result.status, "ok");
    await device.close();
  } finally {
    removeMockDevice(id);
  }
});

Deno.test("control transfers", async () => {
  const id = addMockDevice(testConfig());
  try {
    const device = await findDevice(id);
    await device.open();
    await device.claimInterface(INTERFACE);

    const out = await device.controlTransferOut({
      requestType: "class",
      recipient: "interface",
      request: 0x22,
      value: 0x01,
      index: INTERFACE,
    }, new Uint8Array([0x42]));
    assertEquals(out.status, "ok");
    assertEquals(out.bytesWritten, 1);

    // The mock echoes the setup packet.
    const result = await device.controlTransferIn({
      requestType: "standard",
      recipient: "device",
      request: 0x06,
      value: 0x0100,
      index: 0,
    }, 5);
    assertEquals(result.status, "ok");
    assertEquals(
      [...new Uint8Array(result.data!.buffer, 0, result.data!.byteLength)],
      [0x06, 0x00, 0x01, 0x00, 0x00],
    );
    await device.close();
  } finally {
    removeMockDevice(id);
  }
});

Deno.test("isochronous transfers report NotSupportedError", async () => {
  const id = addMockDevice(testConfig());
  try {
    const device = await findDevice(id);
    await device.open();
    await device.claimInterface(INTERFACE);

    let name = "";
    try {
      await device.isochronousTransferIn(EP_ISO_IN, [64]);
    } catch (error) {
      name = (error as DOMException).name;
    }
    assertEquals(name, "NotSupportedError");

    // Wrong endpoint type is InvalidAccessError, per spec validation order.
    name = "";
    try {
      await device.isochronousTransferIn(EP_IN, [64]);
    } catch (error) {
      name = (error as DOMException).name;
    }
    assertEquals(name, "InvalidAccessError");
    await device.close();
  } finally {
    removeMockDevice(id);
  }
});

Deno.test("connect and disconnect events", async () => {
  const connected = new Promise<USBConnectionEvent>((resolve) => {
    const listener = (event: Event) => {
      usb.removeEventListener("connect", listener);
      resolve(event as USBConnectionEvent);
    };
    usb.addEventListener("connect", listener);
  });

  let disconnectedResolve: (event: USBConnectionEvent) => void;
  const disconnected = new Promise<USBConnectionEvent>((resolve) => {
    disconnectedResolve = resolve;
  });
  const disconnectListener = (event: Event) => {
    usb.removeEventListener("disconnect", disconnectListener);
    disconnectedResolve(event as USBConnectionEvent);
  };
  usb.addEventListener("disconnect", disconnectListener);

  const id = addMockDevice(testConfig());
  const connectEvent = await connected;
  assertEquals(connectEvent.device.id, id);
  assertEquals(connectEvent.device.vendorId, 0x2341);

  removeMockDevice(id);
  const disconnectEvent = await disconnected;
  assertEquals(disconnectEvent.device.id, id);

  // Let the event pump observe the stop and wind down.
  await new Promise((resolve) => setTimeout(resolve, 100));
});
