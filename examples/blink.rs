use webusb::Direction;
use webusb::Result;
use webusb::Usb;
use webusb::UsbControlTransferParameters;
use webusb::UsbDeviceFilter;
use webusb::UsbRecipient;
use webusb::UsbRequestType;

use std::io::BufReader;
use std::io::Read;

const ARDUINO_CONTROL_INIT: UsbControlTransferParameters =
  UsbControlTransferParameters {
    request_type: UsbRequestType::Class,
    recipient: UsbRecipient::Interface,
    request: 0x22,
    value: 0x01,
    index: 2,
  };

const ARDUINO_CONTROL_BYE: UsbControlTransferParameters =
  UsbControlTransferParameters {
    request_type: UsbRequestType::Class,
    recipient: UsbRecipient::Interface,
    request: 0x22,
    value: 0x00,
    index: 2,
  };

fn main() -> Result<()> {
  futures_lite::future::block_on(async {
    let usb = Usb::new()?;

    // Arduino Leonardo.
    let mut device = usb
      .request_device(&[UsbDeviceFilter {
        vendor_id: Some(0x2341),
        product_id: Some(0x8036),
        ..Default::default()
      }])
      .await?;
    device.open().await?;

    device.claim_interface(2).await?;
    device.select_alternate_interface(2, 0).await?;

    device
      .control_transfer_out(ARDUINO_CONTROL_INIT, &[])
      .await?;

    let mut stdin = BufReader::new(std::io::stdin());
    loop {
      let input: Option<u8> =
        (&mut stdin).bytes().next().and_then(|result| result.ok());

      match input {
        Some(b'H') => {
          device.transfer_out(4, b"H").await?;
          device.clear_halt(Direction::Out, 4).await?;
        }
        Some(b'L') => {
          device.transfer_out(4, b"L").await?;
          device.clear_halt(Direction::Out, 4).await?;
        }
        Some(b'Q') => break,
        _ => {}
      }
    }

    device
      .control_transfer_out(ARDUINO_CONTROL_BYE, &[])
      .await?;
    device.close().await?;
    Ok(())
  })
}
