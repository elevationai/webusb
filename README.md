# webusb

Implementation of the [WebUSB specification](https://wicg.github.io/webusb/) in
Rust.

[![Documentation](https://docs.rs/webusb/badge.svg)](https://docs.rs/webusb)
[![Package](https://img.shields.io/crates/v/webusb.svg)](https://crates.io/crates/webusb)

```toml
[dependencies]
webusb = "0.6.0"
```

Built on [nusb](https://github.com/kevinmehall/nusb). All device operations are
async, and the API follows the WebUSB specification step by step:

- Enumeration (`Usb::devices`), filter-based selection (`Usb::request_device`)
  and connect/disconnect events (`Usb::events`).
- Control, bulk and interrupt transfers with spec-shaped results (`ok` / `stall`
  / `babble` statuses instead of errors).
- Full state validation: open/claim checks, alternate settings, `clear_halt`,
  `reset`, and WebUSB platform capability descriptor (landing page URL) parsing.
- Isochronous transfer parameters are validated per the spec but the transfer
  itself returns `Error::NotSupported` until nusb gains isochronous support.

```rust
use webusb::{Usb, UsbDeviceFilter};

futures_lite::future::block_on(async {
  let usb = Usb::new()?;
  let mut device = usb
    .request_device(&[UsbDeviceFilter {
      vendor_id: Some(0x2341),
      ..Default::default()
    }])
    .await?;
  device.open().await?;
  device.claim_interface(2).await?;
  let result = device.transfer_in(5, 64).await?;
  println!("{:?} {:?}", result.status, result.data);
  device.close().await?;
  webusb::Result::Ok(())
})?;
```

### Features

| Feature  | Default | Description                                       |
| -------- | ------- | ------------------------------------------------- |
| `native` | yes     | Real USB devices via nusb.                        |
| `mock`   | no      | In-memory mock backend for hardware-free testing. |
| `ffi`    | no      | C ABI consumed by the Deno bindings.              |
| `serde`  | no      | `Serialize`/`Deserialize` on the data types.      |

### Usage with Deno

Importing `mod.ts` installs a spec-shaped `USB` instance as `navigator.usb`
(including `requestDevice` with filters and `connect`/`disconnect` events).
Build the native library first:

```sh
cargo build --features ffi
deno run --allow-ffi --allow-env your_script.ts
```

```typescript
import "./mod.ts";

// Arduino Leonardo
const device = await navigator.usb.requestDevice({
  filters: [{ vendorId: 0x2341, productId: 0x8036 }],
});

await device.open();
console.log("Device opened.");

if (device.configuration === null) {
  await device.selectConfiguration(1);
}

console.log(`${device.productName} - ${device.serialNumber}`);

await device.claimInterface(2);
await device.selectAlternateInterface(2, 0);
await device.controlTransferOut({
  requestType: "class",
  recipient: "interface",
  request: 0x22,
  value: 0x01,
  index: 2,
});

while (true) {
  const action = prompt(">>");
  if (action === null || action.toLowerCase() === "exit") break;
  const data = new TextEncoder().encode(action);
  await device.transferOut(4, data);
  console.info("Transfer.");
}

await device.close();
console.log("Bye.");
```

Hotplug events:

```typescript
import usb from "./mod.ts";

usb.addEventListener("connect", (event) => {
  console.log("connected:", event.device.productName);
});
usb.addEventListener("disconnect", (event) => {
  console.log("disconnected:", event.device.productName);
});
```

Set `WEBUSB_LIBRARY=/path/to/libwebusb.{so,dylib,dll}` to override where the
native library is loaded from.

### Testing

Tests run against the in-memory mock backend and need no hardware:

```sh
cargo test        # Rust state machine tests
deno task test    # Deno bindings tests (builds the ffi library first)
```

The mock backend (`Usb::mock()` in Rust, `WEBUSB_BACKEND=mock` plus `mock.ts` in
Deno) lets you plug and unplug scripted devices, inspect what was written to
them, and simulate stalls and babble.

To exercise real hardware, load
[this sketch](https://github.com/webusb/arduino/blob/gh-pages/demos/console/sketch/sketch.ino)
into an Arduino Leonardo and run `examples/blink.rs` or
`examples/deno_blink.ts`.

### License

MIT License
