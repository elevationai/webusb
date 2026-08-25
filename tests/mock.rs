use std::collections::HashMap;

use futures_lite::future::block_on;
use webusb::Direction;
use webusb::Error;
use webusb::MockDeviceConfig;
use webusb::Usb;
use webusb::UsbAlternateInterface;
use webusb::UsbConfiguration;
use webusb::UsbConnectionEvent;
use webusb::UsbControlTransferParameters;
use webusb::UsbDevice;
use webusb::UsbDeviceFilter;
use webusb::UsbEndpoint;
use webusb::UsbEndpointType;
use webusb::UsbInterface;
use webusb::UsbRecipient;
use webusb::UsbRequestType;
use webusb::UsbTransferStatus;

const EP_OUT: u8 = 4;
const EP_IN: u8 = 5;
const EP_ISO_IN: u8 = 6;
const INTERFACE: u8 = 2;

fn test_config() -> MockDeviceConfig {
  let alt0 = UsbAlternateInterface {
    alternate_setting: 0,
    interface_class: 0xFF,
    interface_subclass: 0x01,
    interface_protocol: 0x02,
    interface_name: Some("test interface".into()),
    endpoints: vec![
      UsbEndpoint {
        endpoint_number: EP_OUT,
        direction: Direction::Out,
        r#type: UsbEndpointType::Bulk,
        packet_size: 64,
      },
      UsbEndpoint {
        endpoint_number: EP_IN,
        direction: Direction::In,
        r#type: UsbEndpointType::Bulk,
        packet_size: 64,
      },
      UsbEndpoint {
        endpoint_number: EP_ISO_IN,
        direction: Direction::In,
        r#type: UsbEndpointType::Isochronous,
        packet_size: 1023,
      },
    ],
  };
  let alt1 = UsbAlternateInterface {
    alternate_setting: 1,
    endpoints: vec![],
    ..alt0.clone()
  };

  MockDeviceConfig {
    vendor_id: 0x2341,
    product_id: 0x8036,
    manufacturer_name: Some("Arduino LLC".into()),
    product_name: Some("Arduino Leonardo".into()),
    serial_number: Some("TEST123".into()),
    configurations: vec![UsbConfiguration {
      configuration_name: Some("Default".into()),
      configuration_value: 1,
      interfaces: vec![UsbInterface {
        interface_number: INTERFACE,
        alternate: alt0.clone(),
        alternates: vec![alt0, alt1],
        claimed: false,
      }],
    }],
    active_configuration: Some(1),
    url: Some("https://example.com/device".into()),
    in_data: HashMap::from([(EP_IN, b"hello from device".to_vec())]),
    ..Default::default()
  }
}

fn test_device() -> (Usb, webusb::MockController, UsbDevice) {
  let (usb, controller) = Usb::mock();
  controller.add_device(test_config());
  let device = block_on(usb.devices()).unwrap().remove(0);
  (usb, controller, device)
}

fn claim_setup() -> UsbControlTransferParameters {
  UsbControlTransferParameters {
    request_type: UsbRequestType::Class,
    recipient: UsbRecipient::Interface,
    request: 0x22,
    value: 0x01,
    index: INTERFACE as u16,
  }
}

#[test]
fn device_metadata() {
  let (_usb, _controller, device) = test_device();
  assert_eq!(device.vendor_id, 0x2341);
  assert_eq!(device.product_id, 0x8036);
  assert_eq!(device.serial_number.as_deref(), Some("TEST123"));
  assert_eq!(device.url.as_deref(), Some("https://example.com/device"));
  assert!(!device.opened);
  // Active configuration is populated from the config list.
  assert_eq!(
    device.configuration.as_ref().unwrap().configuration_value,
    1
  );
}

#[test]
fn open_close_idempotent() {
  let (_usb, _controller, mut device) = test_device();
  block_on(device.open()).unwrap();
  block_on(device.open()).unwrap();
  assert!(device.opened);
  block_on(device.close()).unwrap();
  block_on(device.close()).unwrap();
  assert!(!device.opened);
}

#[test]
fn invalid_state_before_open() {
  let (_usb, _controller, mut device) = test_device();

  assert_eq!(
    block_on(device.select_configuration(1)).unwrap_err(),
    Error::InvalidState
  );
  assert_eq!(
    block_on(device.claim_interface(INTERFACE)).unwrap_err(),
    Error::InvalidState
  );
  assert_eq!(
    block_on(device.select_alternate_interface(INTERFACE, 0)).unwrap_err(),
    Error::InvalidState
  );
  assert_eq!(
    block_on(device.control_transfer_out(claim_setup(), &[])).unwrap_err(),
    Error::InvalidState
  );
  assert_eq!(
    block_on(device.transfer_out(EP_OUT, b"H")).unwrap_err(),
    Error::InvalidState
  );
  assert_eq!(
    block_on(device.clear_halt(Direction::Out, EP_OUT)).unwrap_err(),
    Error::InvalidState
  );
  assert_eq!(block_on(device.reset()).unwrap_err(), Error::InvalidState);
}

#[test]
fn not_found_errors() {
  let (_usb, _controller, mut device) = test_device();
  block_on(device.open()).unwrap();

  assert_eq!(
    block_on(device.select_configuration(255)).unwrap_err(),
    Error::NotFound
  );
  assert_eq!(
    block_on(device.claim_interface(255)).unwrap_err(),
    Error::NotFound
  );
  assert_eq!(
    block_on(device.release_interface(255)).unwrap_err(),
    Error::NotFound
  );
  assert_eq!(
    block_on(device.select_alternate_interface(255, 0)).unwrap_err(),
    Error::NotFound
  );
  assert_eq!(
    block_on(device.transfer_in(255, 64)).unwrap_err(),
    Error::NotFound
  );
}

#[test]
fn transfers_require_claimed_interface() {
  let (_usb, _controller, mut device) = test_device();
  block_on(device.open()).unwrap();

  // Opened but not claimed: transfers must be rejected.
  assert_eq!(
    block_on(device.transfer_out(EP_OUT, b"H")).unwrap_err(),
    Error::InvalidState
  );
  assert_eq!(
    block_on(device.transfer_in(EP_IN, 64)).unwrap_err(),
    Error::InvalidState
  );

  block_on(device.claim_interface(INTERFACE)).unwrap();
  block_on(device.transfer_out(EP_OUT, b"H")).unwrap();
  block_on(device.transfer_in(EP_IN, 64)).unwrap();
}

#[test]
fn transfer_roundtrip() {
  let (_usb, controller, mut device) = test_device();
  let id = device.id;
  block_on(device.open()).unwrap();
  block_on(device.claim_interface(INTERFACE)).unwrap();

  let out = block_on(device.transfer_out(EP_OUT, b"LED ON")).unwrap();
  assert_eq!(out.status, UsbTransferStatus::Ok);
  assert_eq!(out.bytes_written, 6);
  assert_eq!(
    controller.written(id).unwrap(),
    vec![(EP_OUT, b"LED ON".to_vec())]
  );

  let result = block_on(device.transfer_in(EP_IN, 64)).unwrap();
  assert_eq!(result.status, UsbTransferStatus::Ok);
  assert_eq!(result.data, b"hello from device");

  // Length limits the read.
  let result = block_on(device.transfer_in(EP_IN, 5)).unwrap();
  assert_eq!(result.data, b"hello");
}

#[test]
fn transfer_on_wrong_endpoint_type() {
  let (_usb, _controller, mut device) = test_device();
  block_on(device.open()).unwrap();
  block_on(device.claim_interface(INTERFACE)).unwrap();

  // EP_ISO_IN is isochronous: transfer_in must reject with InvalidAccess.
  assert_eq!(
    block_on(device.transfer_in(EP_ISO_IN, 64)).unwrap_err(),
    Error::InvalidAccess
  );
}

#[test]
fn stall_and_clear_halt() {
  let (_usb, controller, mut device) = test_device();
  let id = device.id;
  block_on(device.open()).unwrap();
  block_on(device.claim_interface(INTERFACE)).unwrap();

  controller.halt_endpoint(id, 0x80 | EP_IN).unwrap();
  let result = block_on(device.transfer_in(EP_IN, 64)).unwrap();
  assert_eq!(result.status, UsbTransferStatus::Stall);
  assert!(result.data.is_empty());

  block_on(device.clear_halt(Direction::In, EP_IN)).unwrap();
  let result = block_on(device.transfer_in(EP_IN, 64)).unwrap();
  assert_eq!(result.status, UsbTransferStatus::Ok);
}

#[test]
fn babble_status() {
  let (usb, controller, _) = {
    let (usb, controller) = Usb::mock();
    let mut config = test_config();
    config.babble_endpoints = vec![0x80 | EP_IN];
    controller.add_device(config);
    (usb, controller, ())
  };
  let mut device = block_on(usb.devices()).unwrap().remove(0);
  block_on(device.open()).unwrap();
  block_on(device.claim_interface(INTERFACE)).unwrap();

  let result = block_on(device.transfer_in(EP_IN, 64)).unwrap();
  assert_eq!(result.status, UsbTransferStatus::Babble);
  drop(controller);
}

#[test]
fn control_transfer_validation() {
  let (_usb, controller, mut device) = test_device();
  let id = device.id;
  block_on(device.open()).unwrap();

  // Interface recipient requires the interface to be claimed.
  assert_eq!(
    block_on(device.control_transfer_out(claim_setup(), &[])).unwrap_err(),
    Error::InvalidState
  );
  // Unknown interface number.
  let mut bad = claim_setup();
  bad.index = 255;
  assert_eq!(
    block_on(device.control_transfer_out(bad, &[])).unwrap_err(),
    Error::NotFound
  );

  block_on(device.claim_interface(INTERFACE)).unwrap();
  let result =
    block_on(device.control_transfer_out(claim_setup(), b"x")).unwrap();
  assert_eq!(result.status, UsbTransferStatus::Ok);
  assert_eq!(result.bytes_written, 1);
  assert_eq!(controller.written(id).unwrap(), vec![(0, b"x".to_vec())]);

  // Endpoint recipient: endpoint must exist.
  let endpoint_setup = UsbControlTransferParameters {
    request_type: UsbRequestType::Standard,
    recipient: UsbRecipient::Endpoint,
    request: 0x01,
    value: 0,
    index: 0x80 | EP_IN as u16,
  };
  let result = block_on(device.control_transfer_in(endpoint_setup, 4)).unwrap();
  assert_eq!(result.status, UsbTransferStatus::Ok);

  let missing_endpoint = UsbControlTransferParameters {
    request_type: UsbRequestType::Standard,
    recipient: UsbRecipient::Endpoint,
    request: 0x01,
    value: 0,
    index: 0x0F,
  };
  assert_eq!(
    block_on(device.control_transfer_in(missing_endpoint, 4)).unwrap_err(),
    Error::NotFound
  );
}

#[test]
fn control_transfer_in_data() {
  let (_usb, _controller, mut device) = test_device();
  block_on(device.open()).unwrap();

  let setup = UsbControlTransferParameters {
    request_type: UsbRequestType::Standard,
    recipient: UsbRecipient::Device,
    request: 0x06,
    value: 0x0100,
    index: 0,
  };
  let result = block_on(device.control_transfer_in(setup, 5)).unwrap();
  assert_eq!(result.status, UsbTransferStatus::Ok);
  // Mock echoes the setup packet: [request, value_lo, value_hi, index_lo, index_hi].
  assert_eq!(result.data, vec![0x06, 0x00, 0x01, 0x00, 0x00]);
}

#[test]
fn isochronous_transfers_validated_then_unsupported() {
  let (_usb, _controller, mut device) = test_device();

  // Not open.
  block_on(device.open()).unwrap();

  // Bulk endpoint is not isochronous.
  assert_eq!(
    block_on(device.isochronous_transfer_in(EP_IN, &[64])).unwrap_err(),
    Error::InvalidAccess
  );
  // Unknown endpoint.
  assert_eq!(
    block_on(device.isochronous_transfer_in(15, &[64])).unwrap_err(),
    Error::NotFound
  );
  // Correct endpoint, interface not claimed.
  assert_eq!(
    block_on(device.isochronous_transfer_in(EP_ISO_IN, &[64])).unwrap_err(),
    Error::InvalidState
  );

  block_on(device.claim_interface(INTERFACE)).unwrap();
  assert_eq!(
    block_on(device.isochronous_transfer_in(EP_ISO_IN, &[64])).unwrap_err(),
    Error::NotSupported
  );
}

#[test]
fn alternate_interface_selection() {
  let (_usb, _controller, mut device) = test_device();
  block_on(device.open()).unwrap();

  // Requires the interface to be claimed.
  assert_eq!(
    block_on(device.select_alternate_interface(INTERFACE, 1)).unwrap_err(),
    Error::InvalidState
  );

  block_on(device.claim_interface(INTERFACE)).unwrap();

  // Unknown alternate setting.
  assert_eq!(
    block_on(device.select_alternate_interface(INTERFACE, 9)).unwrap_err(),
    Error::NotFound
  );

  block_on(device.select_alternate_interface(INTERFACE, 1)).unwrap();
  let configuration = device.configuration.as_ref().unwrap();
  assert_eq!(configuration.interfaces[0].alternate.alternate_setting, 1);
  // Alternate 1 has no endpoints, so the bulk transfer target is gone.
  assert_eq!(
    block_on(device.transfer_in(EP_IN, 64)).unwrap_err(),
    Error::NotFound
  );
}

#[test]
fn close_releases_claimed_interfaces() {
  let (_usb, _controller, mut device) = test_device();
  block_on(device.open()).unwrap();
  block_on(device.claim_interface(INTERFACE)).unwrap();
  block_on(device.close()).unwrap();

  block_on(device.open()).unwrap();
  // The claim did not survive the close.
  assert_eq!(
    block_on(device.transfer_in(EP_IN, 64)).unwrap_err(),
    Error::InvalidState
  );
}

#[test]
fn release_interface_flow() {
  let (_usb, _controller, mut device) = test_device();
  block_on(device.open()).unwrap();
  block_on(device.claim_interface(INTERFACE)).unwrap();
  // Claiming twice is a no-op.
  block_on(device.claim_interface(INTERFACE)).unwrap();

  block_on(device.release_interface(INTERFACE)).unwrap();
  // Releasing twice is a no-op.
  block_on(device.release_interface(INTERFACE)).unwrap();

  assert_eq!(
    block_on(device.transfer_in(EP_IN, 64)).unwrap_err(),
    Error::InvalidState
  );
}

#[test]
fn request_device_filters() {
  let (usb, controller) = Usb::mock();
  controller.add_device(test_config());
  let mut other = test_config();
  other.vendor_id = 0x1234;
  other.product_id = 0x5678;
  other.serial_number = Some("OTHER".into());
  controller.add_device(other);

  // Vendor/product match.
  let device = block_on(usb.request_device(&[UsbDeviceFilter {
    vendor_id: Some(0x1234),
    product_id: Some(0x5678),
    ..Default::default()
  }]))
  .unwrap();
  assert_eq!(device.serial_number.as_deref(), Some("OTHER"));

  // Interface class triple match.
  let device = block_on(usb.request_device(&[UsbDeviceFilter {
    class_code: Some(0xFF),
    subclass_code: Some(0x01),
    protocol_code: Some(0x02),
    ..Default::default()
  }]))
  .unwrap();
  assert_eq!(device.vendor_id, 0x2341);

  // Serial number match.
  let device = block_on(usb.request_device(&[UsbDeviceFilter {
    serial_number: Some("TEST123".into()),
    ..Default::default()
  }]))
  .unwrap();
  assert_eq!(device.vendor_id, 0x2341);

  // Empty filter list matches the first device.
  let device = block_on(usb.request_device(&[])).unwrap();
  assert_eq!(device.vendor_id, 0x2341);

  // No match.
  assert_eq!(
    block_on(usb.request_device(&[UsbDeviceFilter {
      vendor_id: Some(0xDEAD),
      ..Default::default()
    }]))
    .unwrap_err(),
    Error::NotFound
  );
}

#[test]
fn connect_and_disconnect_events() {
  let (usb, controller) = Usb::mock();
  let mut events = usb.events().unwrap();

  let id = controller.add_device(test_config());
  match events.next_blocking().unwrap() {
    UsbConnectionEvent::Connect(device) => {
      assert_eq!(device.id, id);
      assert_eq!(device.vendor_id, 0x2341);
    }
    _ => panic!("expected connect event"),
  }

  controller.remove_device(id).unwrap();
  match events.next_blocking().unwrap() {
    UsbConnectionEvent::Disconnect {
      id: gone,
      vendor_id,
      product_id,
    } => {
      assert_eq!(gone, id);
      assert_eq!(vendor_id, 0x2341);
      assert_eq!(product_id, 0x8036);
    }
    _ => panic!("expected disconnect event"),
  }
}

#[test]
fn disconnected_device_errors() {
  let (usb, controller) = Usb::mock();
  let id = controller.add_device(test_config());
  let mut device = block_on(usb.devices()).unwrap().remove(0);
  block_on(device.open()).unwrap();
  block_on(device.claim_interface(INTERFACE)).unwrap();

  controller.remove_device(id).unwrap();
  assert_eq!(
    block_on(device.transfer_in(EP_IN, 64)).unwrap_err(),
    Error::Disconnected
  );
  // Gone from enumeration too.
  assert!(block_on(usb.devices()).unwrap().is_empty());
}

#[test]
fn device_ids_are_stable_across_enumerations() {
  let (usb, controller) = Usb::mock();
  let id = controller.add_device(test_config());
  let first = block_on(usb.devices()).unwrap().remove(0);
  let second = block_on(usb.devices()).unwrap().remove(0);
  assert_eq!(first.id, id);
  assert_eq!(second.id, id);
}
