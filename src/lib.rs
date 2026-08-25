//! # webusb
//!
//! Implementation of the [WebUSB API specification](https://wicg.github.io/webusb/) in
//! Rust.
//!
//! ## Design
//!
//! The crate is designed to be as close to the WebUSB specification as possible.
//! There are two backends available:
//!
//! - `native` (default): real USB devices via [`nusb`].
//! - `mock`: an in-memory backend for tests and hardware-free development.
//!
//! Entry point is [`Usb`] (the equivalent of `navigator.usb`): enumerate with
//! [`Usb::devices`], select with [`Usb::request_device`], and watch
//! connect/disconnect events with [`Usb::events`].
//!
//! All device operations are `async`. Isochronous transfers are validated but
//! currently return [`Error::NotSupported`] on the native backend because the
//! underlying `nusb` library does not support them yet.
//!
//! see [usbd-webusb](https://github.com/redpfire/usbd-webusb) for WebUSB compatible firmware
//! for the device.
//!
//! ## Usage
//!
//! See [webusb/examples](https://github.com/littledivy/webusb/tree/main/examples) for usage examples.

#[cfg(not(any(feature = "native", feature = "mock")))]
compile_error!(
  "webusb requires at least one backend feature: `native` or `mock`"
);

#[cfg(feature = "serde")]
use serde::Deserialize;
#[cfg(feature = "serde")]
use serde::Serialize;

use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

pub(crate) mod backend;
pub mod constants;
#[cfg(any(feature = "native", test))]
mod descriptors;
#[cfg(feature = "ffi")]
pub mod ffi;

#[cfg(feature = "mock")]
pub use backend::mock::MockController;
#[cfg(feature = "mock")]
pub use backend::mock::MockDeviceConfig;

use backend::BackendDevice;
use backend::TransferOutcome;

pub(crate) const EP_DIR_IN: u8 = 0x80;
pub(crate) const EP_DIR_OUT: u8 = 0x0;

/// Monotonic source of device ids, shared by all backends so ids are unique
/// process-wide and stable for the lifetime of a connected device.
pub(crate) static NEXT_DEVICE_ID: AtomicU64 = AtomicU64::new(1);

pub(crate) fn next_device_id() -> u64 {
  NEXT_DEVICE_ID.fetch_add(1, Ordering::Relaxed)
}

#[derive(Debug, PartialEq, Eq, Clone)]
#[non_exhaustive]
pub enum Error {
  /// Equivalent of DOMException `NotFoundError`.
  NotFound,
  /// Equivalent of DOMException `InvalidStateError`.
  InvalidState,
  /// Equivalent of DOMException `InvalidAccessError`.
  InvalidAccess,
  /// Equivalent of DOMException `NotSupportedError`.
  NotSupported,
  /// The device has been disconnected.
  Disconnected,
  /// The device or interface is in use by another program or driver.
  Busy,
  /// Permission denied by the operating system.
  Access,
  /// Any other OS or transport level error.
  Io(String),
}

impl std::fmt::Display for Error {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    match self {
      Error::NotFound => write!(f, "not found"),
      Error::InvalidState => write!(f, "invalid state"),
      Error::InvalidAccess => write!(f, "invalid access"),
      Error::NotSupported => write!(f, "not supported"),
      Error::Disconnected => write!(f, "device disconnected"),
      Error::Busy => write!(f, "device busy"),
      Error::Access => write!(f, "permission denied"),
      Error::Io(msg) => write!(f, "{}", msg),
    }
  }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Debug)]
#[cfg_attr(
  feature = "serde",
  derive(Serialize, Deserialize),
  serde(rename_all = "camelCase")
)]
pub struct UsbConfiguration {
  /// Name from the string descriptor describing this configuration.
  pub configuration_name: Option<String>,
  /// The configuration number (bConfigurationValue)
  /// https://www.beyondlogic.org/usbnutshell/usb5.shtml#ConfigurationDescriptors
  pub configuration_value: u8,
  pub interfaces: Vec<UsbInterface>,
}

#[derive(Clone, Debug)]
#[cfg_attr(
  feature = "serde",
  derive(Serialize, Deserialize),
  serde(rename_all = "camelCase")
)]
pub struct UsbInterface {
  pub interface_number: u8,
  /// The currently selected alternate setting.
  pub alternate: UsbAlternateInterface,
  pub alternates: Vec<UsbAlternateInterface>,
  #[cfg_attr(feature = "serde", serde(default))]
  pub claimed: bool,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(
  feature = "serde",
  derive(Serialize, Deserialize),
  serde(rename_all = "lowercase")
)]
pub enum UsbEndpointType {
  Bulk,
  Interrupt,
  Isochronous,
  Control,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(
  feature = "serde",
  derive(Serialize, Deserialize),
  serde(rename_all = "lowercase")
)]
pub enum Direction {
  In,
  Out,
}

#[derive(Clone, Debug)]
#[cfg_attr(
  feature = "serde",
  derive(Serialize, Deserialize),
  serde(rename_all = "camelCase")
)]
pub struct UsbEndpoint {
  pub endpoint_number: u8,
  pub direction: Direction,
  pub r#type: UsbEndpointType,
  pub packet_size: u16,
}

#[derive(Clone, Debug)]
#[cfg_attr(
  feature = "serde",
  derive(Serialize, Deserialize),
  serde(rename_all = "camelCase")
)]
pub struct UsbAlternateInterface {
  pub alternate_setting: u8,
  pub interface_class: u8,
  pub interface_subclass: u8,
  pub interface_protocol: u8,
  pub interface_name: Option<String>,
  pub endpoints: Vec<UsbEndpoint>,
}

/// Status of a completed (or failed) transfer.
/// https://wicg.github.io/webusb/#enumdef-usbtransferstatus
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(
  feature = "serde",
  derive(Serialize, Deserialize),
  serde(rename_all = "lowercase")
)]
pub enum UsbTransferStatus {
  Ok,
  Stall,
  Babble,
}

/// https://wicg.github.io/webusb/#usbintransferresult
#[derive(Clone, Debug)]
#[cfg_attr(
  feature = "serde",
  derive(Serialize, Deserialize),
  serde(rename_all = "camelCase")
)]
pub struct UsbInTransferResult {
  pub data: Vec<u8>,
  pub status: UsbTransferStatus,
}

/// https://wicg.github.io/webusb/#usbouttransferresult
#[derive(Clone, Debug)]
#[cfg_attr(
  feature = "serde",
  derive(Serialize, Deserialize),
  serde(rename_all = "camelCase")
)]
pub struct UsbOutTransferResult {
  pub bytes_written: usize,
  pub status: UsbTransferStatus,
}

/// https://wicg.github.io/webusb/#usbisochronousintransferpacket
#[derive(Clone, Debug)]
#[cfg_attr(
  feature = "serde",
  derive(Serialize, Deserialize),
  serde(rename_all = "camelCase")
)]
pub struct UsbIsochronousInTransferPacket {
  pub data: Vec<u8>,
  pub status: UsbTransferStatus,
}

/// https://wicg.github.io/webusb/#usbisochronousintransferresult
#[derive(Clone, Debug)]
#[cfg_attr(
  feature = "serde",
  derive(Serialize, Deserialize),
  serde(rename_all = "camelCase")
)]
pub struct UsbIsochronousInTransferResult {
  pub packets: Vec<UsbIsochronousInTransferPacket>,
}

/// https://wicg.github.io/webusb/#usbisochronousouttransferpacket
#[derive(Clone, Debug)]
#[cfg_attr(
  feature = "serde",
  derive(Serialize, Deserialize),
  serde(rename_all = "camelCase")
)]
pub struct UsbIsochronousOutTransferPacket {
  pub bytes_written: usize,
  pub status: UsbTransferStatus,
}

/// https://wicg.github.io/webusb/#usbisochronousouttransferresult
#[derive(Clone, Debug)]
#[cfg_attr(
  feature = "serde",
  derive(Serialize, Deserialize),
  serde(rename_all = "camelCase")
)]
pub struct UsbIsochronousOutTransferResult {
  pub packets: Vec<UsbIsochronousOutTransferPacket>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(
  feature = "serde",
  derive(Serialize, Deserialize),
  serde(rename_all = "lowercase")
)]
pub enum UsbRequestType {
  Standard,
  Class,
  Vendor,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(
  feature = "serde",
  derive(Serialize, Deserialize),
  serde(rename_all = "lowercase")
)]
pub enum UsbRecipient {
  Device,
  Interface,
  Endpoint,
  Other,
}

#[derive(Clone, Debug)]
#[cfg_attr(
  feature = "serde",
  derive(Serialize, Deserialize),
  serde(rename_all = "camelCase")
)]
pub struct UsbControlTransferParameters {
  pub request_type: UsbRequestType,
  pub recipient: UsbRecipient,
  pub request: u8,
  pub value: u16,
  pub index: u16,
}

/// https://wicg.github.io/webusb/#dictdef-usbdevicefilter
#[derive(Clone, Debug, Default)]
#[cfg_attr(
  feature = "serde",
  derive(Serialize, Deserialize),
  serde(rename_all = "camelCase", default)
)]
pub struct UsbDeviceFilter {
  pub vendor_id: Option<u16>,
  pub product_id: Option<u16>,
  pub class_code: Option<u8>,
  pub subclass_code: Option<u8>,
  pub protocol_code: Option<u8>,
  pub serial_number: Option<String>,
}

impl UsbDeviceFilter {
  /// https://wicg.github.io/webusb/#dfn-match-a-device-filter
  pub fn matches(&self, device: &UsbDevice) -> bool {
    if let Some(vendor_id) = self.vendor_id {
      if device.vendor_id != vendor_id {
        return false;
      }
    }
    if let Some(product_id) = self.product_id {
      if device.product_id != product_id {
        return false;
      }
    }
    if let Some(serial_number) = &self.serial_number {
      if device.serial_number.as_ref() != Some(serial_number) {
        return false;
      }
    }
    match (self.class_code, self.subclass_code, self.protocol_code) {
      (None, None, None) => true,
      _ => {
        // The device descriptor class triple matches, or any interface's
        // class triple matches.
        self.matches_class_triple(
          device.device_class,
          device.device_subclass,
          device.device_protocol,
        ) || device.configurations.iter().any(|config| {
          config.interfaces.iter().any(|itf| {
            itf.alternates.iter().any(|alt| {
              self.matches_class_triple(
                alt.interface_class,
                alt.interface_subclass,
                alt.interface_protocol,
              )
            })
          })
        })
      }
    }
  }

  fn matches_class_triple(
    &self,
    class: u8,
    subclass: u8,
    protocol: u8,
  ) -> bool {
    match self.class_code {
      Some(c) if c != class => return false,
      None => return true,
      _ => {}
    }
    match self.subclass_code {
      Some(s) if s != subclass => return false,
      None => return true,
      _ => {}
    }
    !matches!(self.protocol_code, Some(p) if p != protocol)
  }
}

/// Snapshot of everything a backend knows about a device before it is opened.
pub(crate) struct DeviceData {
  pub configurations: Vec<UsbConfiguration>,
  pub active_configuration_value: Option<u8>,
  pub device_class: u8,
  pub device_subclass: u8,
  pub device_protocol: u8,
  pub device_version_major: u8,
  pub device_version_minor: u8,
  pub device_version_subminor: u8,
  pub manufacturer_name: Option<String>,
  pub product_id: u16,
  pub product_name: Option<String>,
  pub serial_number: Option<String>,
  pub usb_version_major: u8,
  pub usb_version_minor: u8,
  pub usb_version_subminor: u8,
  pub vendor_id: u16,
  pub url: Option<String>,
}

/// Represents a UsbDevice.
/// Obtain one through [`Usb::devices`], [`Usb::request_device`] or a
/// [`UsbConnectionEvent::Connect`] event.
/// https://wicg.github.io/webusb/#device-usage
#[derive(Debug)]
#[cfg_attr(
  feature = "serde",
  derive(Serialize),
  serde(rename_all = "camelCase")
)]
pub struct UsbDevice {
  /// Process-unique identifier for this device, stable while the device
  /// remains connected. Referenced by [`UsbConnectionEvent::Disconnect`].
  pub id: u64,
  /// List of configurations supported by the device.
  /// Populated from the configuration descriptor.
  /// `configurations.len()` SHALL be equal to the
  /// bNumConfigurations field of the device descriptor.
  pub configurations: Vec<UsbConfiguration>,
  /// Represents the currently selected configuration.
  /// One of the elements of `self.configurations`.
  /// None, if the device is not configured.
  pub configuration: Option<UsbConfiguration>,
  /// bDeviceClass value of the device descriptor.
  pub device_class: u8,
  /// bDeviceSubClass value of the device descriptor.
  pub device_subclass: u8,
  /// bDeviceProtocol value of the device descriptor.
  pub device_protocol: u8,
  /// The major version declared by bcdDevice field
  /// such that bcdDevice 0xJJMN represents major version JJ.
  pub device_version_major: u8,
  /// The minor version declared by bcdDevice field
  /// such that bcdDevice 0xJJMN represents minor version M.
  pub device_version_minor: u8,
  /// The subminor version declared by bcdDevice field
  /// such that bcdDevice 0xJJMN represents subminor version N.
  pub device_version_subminor: u8,
  /// Optional property of the string descriptor.
  /// Indexed by the iManufacturer field of device descriptor.
  pub manufacturer_name: Option<String>,
  /// idProduct field of the device descriptor.
  pub product_id: u16,
  /// Optional property of the string descriptor.
  /// Indexed by the iProduct field of device descriptor.
  pub product_name: Option<String>,
  /// Optional property of the string descriptor.
  /// None, if the iSerialNumber field of device descriptor
  /// is 0.
  pub serial_number: Option<String>,
  /// The major version declared by bcdUSB field
  /// such that bcdUSB 0xJJMN represents major version JJ.
  pub usb_version_major: u8,
  /// The minor version declared by bcdUSB field
  /// such that bcdUSB 0xJJMN represents minor version M.
  pub usb_version_minor: u8,
  /// The subminor version declared by bcdUSB field
  /// such that bcdUSB 0xJJMN represents subminor version N.
  pub usb_version_subminor: u8,
  /// idVendor field of the device descriptor.
  /// https://wicg.github.io/webusb/#vendor-id
  pub vendor_id: u16,
  /// If true, the underlying device handle is owned by this object.
  pub opened: bool,
  /// WEBUSB_URL value of the WebUSB Platform Capability Descriptor.
  pub url: Option<String>,

  #[cfg_attr(feature = "serde", serde(skip))]
  backend: BackendDevice,
}

impl UsbDevice {
  pub(crate) fn from_parts(
    id: u64,
    data: DeviceData,
    backend: BackendDevice,
  ) -> Self {
    let configuration = data.active_configuration_value.and_then(|value| {
      data
        .configurations
        .iter()
        .find(|c| c.configuration_value == value)
        .cloned()
    });
    UsbDevice {
      id,
      configurations: data.configurations,
      configuration,
      device_class: data.device_class,
      device_subclass: data.device_subclass,
      device_protocol: data.device_protocol,
      device_version_major: data.device_version_major,
      device_version_minor: data.device_version_minor,
      device_version_subminor: data.device_version_subminor,
      manufacturer_name: data.manufacturer_name,
      product_id: data.product_id,
      product_name: data.product_name,
      serial_number: data.serial_number,
      usb_version_major: data.usb_version_major,
      usb_version_minor: data.usb_version_minor,
      usb_version_subminor: data.usb_version_subminor,
      vendor_id: data.vendor_id,
      opened: false,
      url: data.url,
      backend,
    }
  }

  // https://wicg.github.io/webusb/#check-the-validity-of-the-control-transfer-parameters
  fn validate_control_setup(
    &self,
    setup: &UsbControlTransferParameters,
  ) -> Result<()> {
    match setup.recipient {
      // 4.
      UsbRecipient::Interface => {
        // 4.1
        let interface_number: u8 = (setup.index & 0xFF) as u8;

        // 4.2
        let configuration =
          self.configuration.as_ref().ok_or(Error::NotFound)?;
        let interface = configuration
          .interfaces
          .iter()
          .find(|itf| itf.interface_number == interface_number)
          .ok_or(Error::NotFound)?;

        // 4.3
        if !interface.claimed {
          return Err(Error::InvalidState);
        }
      }
      // 5.
      UsbRecipient::Endpoint => {
        // 5.1
        let endpoint_number = (setup.index & 0x0F) as u8;

        // 5.2
        let direction = match setup.index & 0x80 {
          0 => Direction::Out,
          _ => Direction::In,
        };

        // 5.3-5.4
        let configuration =
          self.configuration.as_ref().ok_or(Error::NotFound)?;
        configuration
          .interfaces
          .iter()
          .find(|itf| {
            itf.alternate.endpoints.iter().any(|endpoint| {
              endpoint.endpoint_number == endpoint_number
                && endpoint.direction == direction
            })
          })
          .ok_or(Error::NotFound)?;
      }
      _ => {}
    }

    Ok(())
  }

  /// Find the endpoint and its owning interface among the currently selected
  /// alternate settings of the active configuration.
  fn find_endpoint(
    &self,
    endpoint_number: u8,
    direction: Direction,
  ) -> Result<(&UsbInterface, &UsbEndpoint)> {
    let configuration = self.configuration.as_ref().ok_or(Error::NotFound)?;
    configuration
      .interfaces
      .iter()
      .find_map(|itf| {
        itf
          .alternate
          .endpoints
          .iter()
          .find(|endpoint| {
            endpoint.endpoint_number == endpoint_number
              && endpoint.direction == direction
          })
          .map(|endpoint| (itf, endpoint))
      })
      .ok_or(Error::NotFound)
  }

  /// https://wicg.github.io/webusb/#dom-usbdevice-open
  pub async fn open(&mut self) -> Result<()> {
    // 3. device is already open?
    if self.opened {
      return Ok(());
    }

    // 4.
    self.backend.open().await?;

    // 5.
    self.opened = true;
    Ok(())
  }

  /// https://wicg.github.io/webusb/#dom-usbdevice-close
  pub async fn close(&mut self) -> Result<()> {
    // 3. device is already closed?
    if !self.opened {
      return Ok(());
    }

    // 5-6. release claimed interfaces, close device and release handle
    self.backend.close().await?;
    if let Some(configuration) = &mut self.configuration {
      for interface in &mut configuration.interfaces {
        interface.claimed = false;
      }
    }

    // 7.
    self.opened = false;
    Ok(())
  }

  /// https://wicg.github.io/webusb/#dom-usbdevice-selectconfiguration
  /// `configuration_value` is the bConfigurationValue of the device configuration.
  pub async fn select_configuration(
    &mut self,
    configuration_value: u8,
  ) -> Result<()> {
    // 3.
    let configuration = self
      .configurations
      .iter()
      .find(|c| c.configuration_value == configuration_value)
      .cloned()
      .ok_or(Error::NotFound)?;

    // 4.
    if !self.opened {
      return Err(Error::InvalidState);
    }

    // 5-6.
    self
      .backend
      .select_configuration(configuration_value)
      .await?;

    // 7.
    self.configuration = Some(configuration);
    Ok(())
  }

  /// https://wicg.github.io/webusb/#dom-usbdevice-claiminterface
  pub async fn claim_interface(&mut self, interface_number: u8) -> Result<()> {
    // 2.
    let active_configuration =
      self.configuration.as_mut().ok_or(Error::NotFound)?;
    let interface = active_configuration
      .interfaces
      .iter_mut()
      .find(|i| i.interface_number == interface_number)
      .ok_or(Error::NotFound)?;

    // 3.
    if !self.opened {
      return Err(Error::InvalidState);
    }

    // 4.
    if interface.claimed {
      return Ok(());
    }

    // 5-6.
    self.backend.claim_interface(interface_number).await?;
    // Re-borrow: the backend call required releasing the earlier borrow.
    if let Some(configuration) = &mut self.configuration {
      if let Some(interface) = configuration
        .interfaces
        .iter_mut()
        .find(|i| i.interface_number == interface_number)
      {
        interface.claimed = true;
      }
    }

    Ok(())
  }

  /// https://wicg.github.io/webusb/#dom-usbdevice-releaseinterface
  pub async fn release_interface(
    &mut self,
    interface_number: u8,
  ) -> Result<()> {
    // 3.
    let active_configuration =
      self.configuration.as_mut().ok_or(Error::NotFound)?;
    let interface = active_configuration
      .interfaces
      .iter_mut()
      .find(|i| i.interface_number == interface_number)
      .ok_or(Error::NotFound)?;

    // 4.
    if !self.opened {
      return Err(Error::InvalidState);
    }

    // 5.
    if !interface.claimed {
      return Ok(());
    }

    // 6-7.
    self.backend.release_interface(interface_number).await?;
    if let Some(configuration) = &mut self.configuration {
      if let Some(interface) = configuration
        .interfaces
        .iter_mut()
        .find(|i| i.interface_number == interface_number)
      {
        interface.claimed = false;
      }
    }

    Ok(())
  }

  /// https://wicg.github.io/webusb/#dom-usbdevice-selectalternateinterface
  pub async fn select_alternate_interface(
    &mut self,
    interface_number: u8,
    alternate_setting: u8,
  ) -> Result<()> {
    // 3.
    let active_configuration =
      self.configuration.as_ref().ok_or(Error::NotFound)?;
    let interface = active_configuration
      .interfaces
      .iter()
      .find(|i| i.interface_number == interface_number)
      .ok_or(Error::NotFound)?;

    // 4.
    if !self.opened || !interface.claimed {
      return Err(Error::InvalidState);
    }

    // 5.
    let alternate = interface
      .alternates
      .iter()
      .find(|alt| alt.alternate_setting == alternate_setting)
      .cloned()
      .ok_or(Error::NotFound)?;

    // 6.
    self
      .backend
      .select_alternate_interface(interface_number, alternate_setting)
      .await?;

    // 7.
    if let Some(configuration) = &mut self.configuration {
      if let Some(interface) = configuration
        .interfaces
        .iter_mut()
        .find(|i| i.interface_number == interface_number)
      {
        interface.alternate = alternate;
      }
    }
    Ok(())
  }

  /// https://wicg.github.io/webusb/#dom-usbdevice-controltransferin
  pub async fn control_transfer_in(
    &mut self,
    setup: UsbControlTransferParameters,
    length: u16,
  ) -> Result<UsbInTransferResult> {
    // 3.
    if !self.opened {
      return Err(Error::InvalidState);
    }

    // 4.
    self.validate_control_setup(&setup)?;

    // 5-13.
    match self.backend.control_transfer_in(&setup, length).await? {
      TransferOutcome::Ok(data) => Ok(UsbInTransferResult {
        data,
        status: UsbTransferStatus::Ok,
      }),
      TransferOutcome::Stall => Ok(UsbInTransferResult {
        data: Vec::new(),
        status: UsbTransferStatus::Stall,
      }),
      TransferOutcome::Babble => Ok(UsbInTransferResult {
        data: Vec::new(),
        status: UsbTransferStatus::Babble,
      }),
    }
  }

  /// https://wicg.github.io/webusb/#dom-usbdevice-controltransferout
  pub async fn control_transfer_out(
    &mut self,
    setup: UsbControlTransferParameters,
    data: &[u8],
  ) -> Result<UsbOutTransferResult> {
    // 2.
    if !self.opened {
      return Err(Error::InvalidState);
    }

    // 3.
    self.validate_control_setup(&setup)?;

    // 4-9.
    match self.backend.control_transfer_out(&setup, data).await? {
      TransferOutcome::Ok(bytes_written) => Ok(UsbOutTransferResult {
        bytes_written,
        status: UsbTransferStatus::Ok,
      }),
      TransferOutcome::Stall => Ok(UsbOutTransferResult {
        bytes_written: 0,
        status: UsbTransferStatus::Stall,
      }),
      TransferOutcome::Babble => Ok(UsbOutTransferResult {
        bytes_written: 0,
        status: UsbTransferStatus::Babble,
      }),
    }
  }

  /// https://wicg.github.io/webusb/#dom-usbdevice-clearhalt
  pub async fn clear_halt(
    &mut self,
    direction: Direction,
    endpoint_number: u8,
  ) -> Result<()> {
    // 2.
    let (interface, endpoint) =
      self.find_endpoint(endpoint_number, direction)?;
    let interface_number = interface.interface_number;
    let endpoint_type = endpoint.r#type;

    // 3.
    if !self.opened || !interface.claimed {
      return Err(Error::InvalidState);
    }

    // 4-5.
    self
      .backend
      .clear_halt(interface_number, endpoint_type, direction, endpoint_number)
      .await
  }

  /// https://wicg.github.io/webusb/#dom-usbdevice-transferin
  pub async fn transfer_in(
    &mut self,
    endpoint_number: u8,
    length: usize,
  ) -> Result<UsbInTransferResult> {
    // 3.
    let (interface, endpoint) =
      self.find_endpoint(endpoint_number, Direction::In)?;
    let interface_number = interface.interface_number;
    let endpoint_type = endpoint.r#type;
    let claimed = interface.claimed;

    // 4.
    match endpoint_type {
      UsbEndpointType::Bulk | UsbEndpointType::Interrupt => {}
      _ => return Err(Error::InvalidAccess),
    }

    // 5.
    if !self.opened || !claimed {
      return Err(Error::InvalidState);
    }

    // 6-15.
    match self
      .backend
      .transfer_in(interface_number, endpoint_type, endpoint_number, length)
      .await?
    {
      TransferOutcome::Ok(data) => Ok(UsbInTransferResult {
        data,
        status: UsbTransferStatus::Ok,
      }),
      TransferOutcome::Stall => Ok(UsbInTransferResult {
        data: Vec::new(),
        status: UsbTransferStatus::Stall,
      }),
      TransferOutcome::Babble => Ok(UsbInTransferResult {
        data: Vec::new(),
        status: UsbTransferStatus::Babble,
      }),
    }
  }

  /// https://wicg.github.io/webusb/#dom-usbdevice-transferout
  pub async fn transfer_out(
    &mut self,
    endpoint_number: u8,
    data: &[u8],
  ) -> Result<UsbOutTransferResult> {
    // 2.
    let (interface, endpoint) =
      self.find_endpoint(endpoint_number, Direction::Out)?;
    let interface_number = interface.interface_number;
    let endpoint_type = endpoint.r#type;
    let claimed = interface.claimed;

    // 3.
    match endpoint_type {
      UsbEndpointType::Bulk | UsbEndpointType::Interrupt => {}
      _ => return Err(Error::InvalidAccess),
    }

    // 4.
    if !self.opened || !claimed {
      return Err(Error::InvalidState);
    }

    // 5-9.
    match self
      .backend
      .transfer_out(interface_number, endpoint_type, endpoint_number, data)
      .await?
    {
      TransferOutcome::Ok(bytes_written) => Ok(UsbOutTransferResult {
        bytes_written,
        status: UsbTransferStatus::Ok,
      }),
      TransferOutcome::Stall => Ok(UsbOutTransferResult {
        bytes_written: 0,
        status: UsbTransferStatus::Stall,
      }),
      TransferOutcome::Babble => Ok(UsbOutTransferResult {
        bytes_written: 0,
        status: UsbTransferStatus::Babble,
      }),
    }
  }

  /// https://wicg.github.io/webusb/#dom-usbdevice-isochronoustransferin
  ///
  /// Parameters are validated per the specification, but the transfer itself
  /// is currently unsupported (`nusb` does not implement isochronous
  /// transfers yet) and returns [`Error::NotSupported`].
  pub async fn isochronous_transfer_in(
    &mut self,
    endpoint_number: u8,
    packet_lengths: &[u32],
  ) -> Result<UsbIsochronousInTransferResult> {
    let (interface, endpoint) =
      self.find_endpoint(endpoint_number, Direction::In)?;
    let claimed = interface.claimed;

    if endpoint.r#type != UsbEndpointType::Isochronous {
      return Err(Error::InvalidAccess);
    }

    if !self.opened || !claimed {
      return Err(Error::InvalidState);
    }

    let _ = packet_lengths;
    Err(Error::NotSupported)
  }

  /// https://wicg.github.io/webusb/#dom-usbdevice-isochronoustransferout
  ///
  /// Parameters are validated per the specification, but the transfer itself
  /// is currently unsupported (`nusb` does not implement isochronous
  /// transfers yet) and returns [`Error::NotSupported`].
  pub async fn isochronous_transfer_out(
    &mut self,
    endpoint_number: u8,
    data: &[u8],
    packet_lengths: &[u32],
  ) -> Result<UsbIsochronousOutTransferResult> {
    let (interface, endpoint) =
      self.find_endpoint(endpoint_number, Direction::Out)?;
    let claimed = interface.claimed;

    if endpoint.r#type != UsbEndpointType::Isochronous {
      return Err(Error::InvalidAccess);
    }

    if !self.opened || !claimed {
      return Err(Error::InvalidState);
    }

    let _ = (data, packet_lengths);
    Err(Error::NotSupported)
  }

  /// https://wicg.github.io/webusb/#dom-usbdevice-reset
  pub async fn reset(&mut self) -> Result<()> {
    // 3.
    if !self.opened {
      return Err(Error::InvalidState);
    }

    // 4-6.
    self.backend.reset().await
  }
}

/// A connect or disconnect event, yielded by [`UsbEvents`].
/// https://wicg.github.io/webusb/#events
#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
pub enum UsbConnectionEvent {
  Connect(UsbDevice),
  Disconnect {
    /// The [`UsbDevice::id`] of the disconnected device.
    id: u64,
    vendor_id: u16,
    product_id: u16,
  },
}

/// Stream of [`UsbConnectionEvent`]s. Obtained from [`Usb::events`].
pub struct UsbEvents(pub(crate) UsbEventsInner);

pub(crate) enum UsbEventsInner {
  #[cfg(feature = "native")]
  Native(backend::native::NativeEvents),
  #[cfg(feature = "mock")]
  Mock(async_channel::Receiver<UsbConnectionEvent>),
}

impl UsbEvents {
  /// Wait for the next connect/disconnect event. Returns `None` if the
  /// event source has been closed.
  pub async fn next(&mut self) -> Option<UsbConnectionEvent> {
    match &mut self.0 {
      #[cfg(feature = "native")]
      UsbEventsInner::Native(events) => events.next().await,
      #[cfg(feature = "mock")]
      UsbEventsInner::Mock(receiver) => receiver.recv().await.ok(),
    }
  }

  /// Blocking variant of [`UsbEvents::next`].
  pub fn next_blocking(&mut self) -> Option<UsbConnectionEvent> {
    futures_lite::future::block_on(self.next())
  }
}

/// The WebUSB entry point, equivalent of `navigator.usb`.
/// https://wicg.github.io/webusb/#usb
pub struct Usb(pub(crate) UsbInner);

pub(crate) enum UsbInner {
  #[cfg(feature = "native")]
  Native,
  #[cfg(feature = "mock")]
  Mock(backend::mock::MockHubRef),
}

impl Usb {
  /// A `Usb` backed by the operating system's real USB devices.
  #[cfg(feature = "native")]
  pub fn new() -> Result<Self> {
    Ok(Usb(UsbInner::Native))
  }

  /// A `Usb` backed by an in-memory mock hub, along with a controller used
  /// to plug and unplug mock devices.
  #[cfg(feature = "mock")]
  pub fn mock() -> (Self, MockController) {
    let hub = backend::mock::MockHubRef::default();
    (Usb(UsbInner::Mock(hub.clone())), MockController::new(hub))
  }

  /// https://wicg.github.io/webusb/#dom-usb-getdevices
  pub async fn devices(&self) -> Result<Vec<UsbDevice>> {
    match &self.0 {
      #[cfg(feature = "native")]
      UsbInner::Native => backend::native::enumerate().await,
      #[cfg(feature = "mock")]
      UsbInner::Mock(hub) => Ok(hub.enumerate()),
    }
  }

  /// https://wicg.github.io/webusb/#dom-usb-requestdevice
  ///
  /// There is no permission chooser outside a browser: the first device
  /// matching any of `filters` is returned. An empty filter list matches
  /// every device.
  pub async fn request_device(
    &self,
    filters: &[UsbDeviceFilter],
  ) -> Result<UsbDevice> {
    let devices = self.devices().await?;
    devices
      .into_iter()
      .find(|device| {
        filters.is_empty() || filters.iter().any(|f| f.matches(device))
      })
      .ok_or(Error::NotFound)
  }

  /// Subscribe to connect/disconnect events.
  /// Equivalent of the `connect`/`disconnect` events on `navigator.usb`.
  pub fn events(&self) -> Result<UsbEvents> {
    match &self.0 {
      #[cfg(feature = "native")]
      UsbInner::Native => Ok(UsbEvents(UsbEventsInner::Native(
        backend::native::NativeEvents::new()?,
      ))),
      #[cfg(feature = "mock")]
      UsbInner::Mock(hub) => {
        Ok(UsbEvents(UsbEventsInner::Mock(hub.subscribe())))
      }
    }
  }
}
