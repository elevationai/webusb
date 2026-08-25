//! Backend for real USB devices, built on [`nusb`].

use std::collections::HashMap;
use std::num::NonZeroU8;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::Duration;

use futures_lite::StreamExt;
use nusb::hotplug::HotplugEvent;
use nusb::transfer::Buffer;
use nusb::transfer::Bulk;
use nusb::transfer::BulkOrInterrupt;
use nusb::transfer::Completion;
use nusb::transfer::ControlIn;
use nusb::transfer::ControlOut;
use nusb::transfer::ControlType;
use nusb::transfer::In;
use nusb::transfer::Interrupt;
use nusb::transfer::Out;
use nusb::transfer::Recipient;
use nusb::transfer::TransferError;

use super::BackendDevice;
use super::TransferOutcome;
#[cfg(not(target_os = "windows"))]
use crate::constants::BOS_DESCRIPTOR_TYPE;
#[cfg(not(target_os = "windows"))]
use crate::constants::GET_URL_REQUEST;
#[cfg(not(target_os = "windows"))]
use crate::descriptors::parse_bos;
#[cfg(not(target_os = "windows"))]
use crate::descriptors::parse_webusb_url;
use crate::next_device_id;
use crate::DeviceData;
use crate::Direction;
use crate::Error;
use crate::Result;
use crate::UsbAlternateInterface;
use crate::UsbConfiguration;
use crate::UsbConnectionEvent;
use crate::UsbControlTransferParameters;
use crate::UsbDevice;
use crate::UsbEndpoint;
use crate::UsbEndpointType;
use crate::UsbInterface;
use crate::UsbRecipient;
use crate::UsbRequestType;
use crate::EP_DIR_IN;
use crate::EP_DIR_OUT;

/// Timeout for descriptor reads performed during enumeration.
const ENUMERATION_TIMEOUT: Duration = Duration::from_secs(2);
/// Timeout for user-initiated control transfers. The WebUSB specification has
/// no timeouts, but the platform APIs require a finite value.
const CONTROL_TIMEOUT: Duration = Duration::from_secs(300);
/// USB hub class code; hubs are not listed, matching browser behavior.
const HUB_CLASS: u8 = 9;
#[cfg(not(target_os = "windows"))]
const GET_DESCRIPTOR_REQUEST: u8 = 0x06;
const DEFAULT_LANGUAGE_ID: u16 = 0x0409;

/// Maps nusb device ids to our process-wide ids so a device keeps the same
/// [`UsbDevice::id`] across enumerations and hotplug events.
struct RegistryEntry {
  id: u64,
  vendor_id: u16,
  product_id: u16,
}

fn registry() -> &'static Mutex<HashMap<nusb::DeviceId, RegistryEntry>> {
  static REGISTRY: OnceLock<Mutex<HashMap<nusb::DeviceId, RegistryEntry>>> =
    OnceLock::new();
  REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

fn register(info: &nusb::DeviceInfo) -> u64 {
  let mut registry = registry().lock().unwrap();
  registry
    .entry(info.id())
    .or_insert_with(|| RegistryEntry {
      id: next_device_id(),
      vendor_id: info.vendor_id(),
      product_id: info.product_id(),
    })
    .id
}

fn map_err(err: nusb::Error) -> Error {
  match err.kind() {
    nusb::ErrorKind::Disconnected => Error::Disconnected,
    nusb::ErrorKind::Busy => Error::Busy,
    nusb::ErrorKind::PermissionDenied => Error::Access,
    nusb::ErrorKind::NotFound => Error::NotFound,
    nusb::ErrorKind::Unsupported => Error::NotSupported,
    _ => Error::Io(err.to_string()),
  }
}

fn map_transfer_err(err: TransferError) -> Error {
  match err {
    TransferError::Disconnected => Error::Disconnected,
    TransferError::InvalidArgument => Error::NotSupported,
    other => Error::Io(other.to_string()),
  }
}

pub(crate) struct NativeDevice {
  info: nusb::DeviceInfo,
  device: Option<nusb::Device>,
  interfaces: HashMap<u8, nusb::Interface>,
}

impl NativeDevice {
  fn device(&self) -> Result<&nusb::Device> {
    self.device.as_ref().ok_or(Error::InvalidState)
  }

  fn interface(&self, interface_number: u8) -> Result<&nusb::Interface> {
    self
      .interfaces
      .get(&interface_number)
      .ok_or(Error::InvalidState)
  }

  pub(crate) async fn open(&mut self) -> Result<()> {
    let device = self.info.open().await.map_err(map_err)?;
    self.device = Some(device);
    Ok(())
  }

  pub(crate) async fn close(&mut self) -> Result<()> {
    // Dropping the interfaces releases them; dropping the device closes it.
    self.interfaces.clear();
    self.device = None;
    Ok(())
  }

  pub(crate) async fn select_configuration(
    &mut self,
    configuration_value: u8,
  ) -> Result<()> {
    self
      .device()?
      .set_configuration(configuration_value)
      .await
      .map_err(map_err)
  }

  pub(crate) async fn claim_interface(
    &mut self,
    interface_number: u8,
  ) -> Result<()> {
    let interface = self
      .device()?
      .claim_interface(interface_number)
      .await
      .map_err(map_err)?;
    self.interfaces.insert(interface_number, interface);
    Ok(())
  }

  pub(crate) async fn release_interface(
    &mut self,
    interface_number: u8,
  ) -> Result<()> {
    match self.interfaces.remove(&interface_number) {
      Some(interface) => interface.release().await.map_err(map_err),
      None => Ok(()),
    }
  }

  pub(crate) async fn select_alternate_interface(
    &mut self,
    interface_number: u8,
    alternate_setting: u8,
  ) -> Result<()> {
    self
      .interface(interface_number)?
      .set_alt_setting(alternate_setting)
      .await
      .map_err(map_err)
  }

  pub(crate) async fn control_transfer_in(
    &mut self,
    setup: &UsbControlTransferParameters,
    length: u16,
  ) -> Result<TransferOutcome<Vec<u8>>> {
    let data = ControlIn {
      control_type: control_type(&setup.request_type),
      recipient: recipient(&setup.recipient),
      request: setup.request,
      value: setup.value,
      index: setup.index,
      length,
    };

    let result = match self.control_target(setup)? {
      Some(interface) => interface.control_in(data, CONTROL_TIMEOUT).await,
      None => self.device_control_in(data).await?,
    };

    match result {
      Ok(bytes) => Ok(TransferOutcome::Ok(bytes)),
      Err(TransferError::Stall) => Ok(TransferOutcome::Stall),
      Err(err) => Err(map_transfer_err(err)),
    }
  }

  #[cfg(not(target_os = "windows"))]
  async fn device_control_in(
    &self,
    data: ControlIn,
  ) -> Result<std::result::Result<Vec<u8>, TransferError>> {
    Ok(self.device()?.control_in(data, CONTROL_TIMEOUT).await)
  }

  #[cfg(target_os = "windows")]
  async fn device_control_in(
    &self,
    _data: ControlIn,
  ) -> Result<std::result::Result<Vec<u8>, TransferError>> {
    // Unreachable: `control_target` always yields an interface or errors
    // on Windows.
    Err(Error::NotSupported)
  }

  pub(crate) async fn control_transfer_out(
    &mut self,
    setup: &UsbControlTransferParameters,
    data: &[u8],
  ) -> Result<TransferOutcome<usize>> {
    let control = ControlOut {
      control_type: control_type(&setup.request_type),
      recipient: recipient(&setup.recipient),
      request: setup.request,
      value: setup.value,
      index: setup.index,
      data,
    };

    let result = match self.control_target(setup)? {
      Some(interface) => interface.control_out(control, CONTROL_TIMEOUT).await,
      None => self.device_control_out(control).await?,
    };

    match result {
      Ok(()) => Ok(TransferOutcome::Ok(data.len())),
      Err(TransferError::Stall) => Ok(TransferOutcome::Stall),
      Err(err) => Err(map_transfer_err(err)),
    }
  }

  #[cfg(not(target_os = "windows"))]
  async fn device_control_out(
    &self,
    data: ControlOut<'_>,
  ) -> Result<std::result::Result<(), TransferError>> {
    Ok(self.device()?.control_out(data, CONTROL_TIMEOUT).await)
  }

  #[cfg(target_os = "windows")]
  async fn device_control_out(
    &self,
    _data: ControlOut<'_>,
  ) -> Result<std::result::Result<(), TransferError>> {
    // Unreachable: `control_target` always yields an interface or errors
    // on Windows.
    Err(Error::NotSupported)
  }

  /// The interface handle a control transfer should go through, or `None`
  /// for the device-level default control endpoint.
  ///
  /// Interface-recipient requests use the claimed interface handle. On
  /// Windows there are no device-level control transfers (WinUSB requires
  /// an interface handle), so any claimed interface is used as a fallback;
  /// with nothing claimed the transfer fails with `NotSupported`.
  fn control_target(
    &self,
    setup: &UsbControlTransferParameters,
  ) -> Result<Option<&nusb::Interface>> {
    if let UsbRecipient::Interface = setup.recipient {
      let interface = self.interfaces.get(&((setup.index & 0xFF) as u8));
      if interface.is_some() {
        return Ok(interface);
      }
    }
    if cfg!(target_os = "windows") {
      match self.interfaces.values().next() {
        Some(interface) => Ok(Some(interface)),
        None => Err(Error::NotSupported),
      }
    } else {
      Ok(None)
    }
  }

  pub(crate) async fn clear_halt(
    &mut self,
    interface_number: u8,
    endpoint_type: UsbEndpointType,
    direction: Direction,
    endpoint_number: u8,
  ) -> Result<()> {
    let interface = self.interface(interface_number)?;
    let address = endpoint_address(direction, endpoint_number);
    match (endpoint_type, direction) {
      (UsbEndpointType::Bulk, Direction::In) => {
        let mut endpoint =
          interface.endpoint::<Bulk, In>(address).map_err(map_err)?;
        endpoint.clear_halt().await.map_err(map_err)
      }
      (UsbEndpointType::Bulk, Direction::Out) => {
        let mut endpoint =
          interface.endpoint::<Bulk, Out>(address).map_err(map_err)?;
        endpoint.clear_halt().await.map_err(map_err)
      }
      (UsbEndpointType::Interrupt, Direction::In) => {
        let mut endpoint = interface
          .endpoint::<Interrupt, In>(address)
          .map_err(map_err)?;
        endpoint.clear_halt().await.map_err(map_err)
      }
      (UsbEndpointType::Interrupt, Direction::Out) => {
        let mut endpoint = interface
          .endpoint::<Interrupt, Out>(address)
          .map_err(map_err)?;
        endpoint.clear_halt().await.map_err(map_err)
      }
      _ => Err(Error::NotSupported),
    }
  }

  pub(crate) async fn transfer_in(
    &mut self,
    interface_number: u8,
    endpoint_type: UsbEndpointType,
    endpoint_number: u8,
    length: usize,
  ) -> Result<TransferOutcome<Vec<u8>>> {
    let interface = self.interface(interface_number)?;
    let address = endpoint_address(Direction::In, endpoint_number);
    match endpoint_type {
      UsbEndpointType::Bulk => {
        transfer_in::<Bulk>(interface, address, length).await
      }
      UsbEndpointType::Interrupt => {
        transfer_in::<Interrupt>(interface, address, length).await
      }
      _ => Err(Error::InvalidAccess),
    }
  }

  pub(crate) async fn transfer_out(
    &mut self,
    interface_number: u8,
    endpoint_type: UsbEndpointType,
    endpoint_number: u8,
    data: &[u8],
  ) -> Result<TransferOutcome<usize>> {
    let interface = self.interface(interface_number)?;
    let address = endpoint_address(Direction::Out, endpoint_number);
    match endpoint_type {
      UsbEndpointType::Bulk => {
        transfer_out::<Bulk>(interface, address, data).await
      }
      UsbEndpointType::Interrupt => {
        transfer_out::<Interrupt>(interface, address, data).await
      }
      _ => Err(Error::InvalidAccess),
    }
  }

  pub(crate) async fn reset(&mut self) -> Result<()> {
    // A reset invalidates claimed interfaces.
    self.interfaces.clear();
    self.device()?.reset().await.map_err(map_err)
  }
}

fn endpoint_address(direction: Direction, endpoint_number: u8) -> u8 {
  match direction {
    Direction::In => EP_DIR_IN | endpoint_number,
    Direction::Out => EP_DIR_OUT | endpoint_number,
  }
}

fn control_type(request_type: &UsbRequestType) -> ControlType {
  match request_type {
    UsbRequestType::Standard => ControlType::Standard,
    UsbRequestType::Class => ControlType::Class,
    UsbRequestType::Vendor => ControlType::Vendor,
  }
}

fn recipient(usb_recipient: &UsbRecipient) -> Recipient {
  match usb_recipient {
    UsbRecipient::Device => Recipient::Device,
    UsbRecipient::Interface => Recipient::Interface,
    UsbRecipient::Endpoint => Recipient::Endpoint,
    UsbRecipient::Other => Recipient::Other,
  }
}

async fn transfer_in<EpType: BulkOrInterrupt>(
  interface: &nusb::Interface,
  address: u8,
  length: usize,
) -> Result<TransferOutcome<Vec<u8>>> {
  let mut endpoint =
    interface.endpoint::<EpType, In>(address).map_err(map_err)?;
  endpoint.submit(Buffer::new(length));
  let completion = endpoint.next_complete().await;
  completion_in(completion)
}

async fn transfer_out<EpType: BulkOrInterrupt>(
  interface: &nusb::Interface,
  address: u8,
  data: &[u8],
) -> Result<TransferOutcome<usize>> {
  let mut endpoint = interface
    .endpoint::<EpType, Out>(address)
    .map_err(map_err)?;
  endpoint.submit(data.to_vec().into());
  let completion = endpoint.next_complete().await;
  match completion.status {
    Ok(()) => Ok(TransferOutcome::Ok(completion.actual_len)),
    Err(TransferError::Stall) => Ok(TransferOutcome::Stall),
    Err(err) => Err(map_transfer_err(err)),
  }
}

fn completion_in(completion: Completion) -> Result<TransferOutcome<Vec<u8>>> {
  match completion.status {
    Ok(()) => {
      let mut data = completion.buffer.into_vec();
      data.truncate(completion.actual_len);
      Ok(TransferOutcome::Ok(data))
    }
    Err(TransferError::Stall) => Ok(TransferOutcome::Stall),
    Err(err) => Err(map_transfer_err(err)),
  }
}

/// Enumerate connected devices. Devices that cannot be opened (e.g. due to
/// permissions) are still listed, with empty configuration data.
pub(crate) async fn enumerate() -> Result<Vec<UsbDevice>> {
  let devices = nusb::list_devices().await.map_err(map_err)?;
  let mut usb_devices = Vec::new();
  for info in devices {
    // Do not list hubs, matching browser WebUSB behavior.
    if info.class() == HUB_CLASS {
      continue;
    }
    usb_devices.push(build_device(info).await);
  }
  Ok(usb_devices)
}

pub(crate) async fn build_device(info: nusb::DeviceInfo) -> UsbDevice {
  let id = register(&info);

  let usb_version = info.usb_version();
  let device_version = info.device_version();

  let mut configurations = Vec::new();
  let mut active_configuration_value = None;
  let mut url = None;

  if let Ok(device) = info.open().await {
    for descriptor in device.configurations() {
      configurations.push(convert_configuration(&device, descriptor).await);
    }
    active_configuration_value = device
      .active_configuration()
      .ok()
      .map(|c| c.configuration_value());
    // The WebUSB platform capability descriptor requires USB 2.1 or later.
    if usb_version >= 0x0210 {
      url = read_webusb_url(&device).await;
    }
  }

  let data = DeviceData {
    configurations,
    active_configuration_value,
    device_class: info.class(),
    device_subclass: info.subclass(),
    device_protocol: info.protocol(),
    device_version_major: (device_version >> 8) as u8,
    device_version_minor: ((device_version >> 4) & 0xF) as u8,
    device_version_subminor: (device_version & 0xF) as u8,
    manufacturer_name: info.manufacturer_string().map(str::to_string),
    product_id: info.product_id(),
    product_name: info.product_string().map(str::to_string),
    serial_number: info.serial_number().map(str::to_string),
    usb_version_major: (usb_version >> 8) as u8,
    usb_version_minor: ((usb_version >> 4) & 0xF) as u8,
    usb_version_subminor: (usb_version & 0xF) as u8,
    vendor_id: info.vendor_id(),
    url,
  };

  let backend = BackendDevice::Native(Box::new(NativeDevice {
    info,
    device: None,
    interfaces: HashMap::new(),
  }));

  UsbDevice::from_parts(id, data, backend)
}

async fn convert_configuration(
  device: &nusb::Device,
  descriptor: nusb::descriptors::ConfigurationDescriptor<'_>,
) -> UsbConfiguration {
  let configuration_name = match descriptor.string_index() {
    Some(index) => read_string(device, index).await,
    None => None,
  };

  let mut interfaces = Vec::new();
  for interface in descriptor.interfaces() {
    let mut alternates = Vec::new();
    for alt in interface.alt_settings() {
      alternates.push(convert_alternate(device, alt).await);
    }
    // The default alternate setting is the one with bAlternateSetting 0,
    // falling back to the first listed.
    let alternate = alternates
      .iter()
      .find(|a| a.alternate_setting == 0)
      .or_else(|| alternates.first())
      .cloned();
    let alternate = match alternate {
      Some(alternate) => alternate,
      None => continue,
    };
    interfaces.push(UsbInterface {
      interface_number: interface.interface_number(),
      alternate,
      alternates,
      claimed: false,
    });
  }

  UsbConfiguration {
    configuration_name,
    configuration_value: descriptor.configuration_value(),
    interfaces,
  }
}

async fn convert_alternate(
  device: &nusb::Device,
  descriptor: nusb::descriptors::InterfaceDescriptor<'_>,
) -> UsbAlternateInterface {
  let interface_name = match descriptor.string_index() {
    Some(index) => read_string(device, index).await,
    None => None,
  };
  UsbAlternateInterface {
    alternate_setting: descriptor.alternate_setting(),
    interface_class: descriptor.class(),
    interface_subclass: descriptor.subclass(),
    interface_protocol: descriptor.protocol(),
    interface_name,
    endpoints: descriptor
      .endpoints()
      .map(|endpoint| UsbEndpoint {
        endpoint_number: endpoint.address() & 0x0F,
        direction: match endpoint.direction() {
          nusb::transfer::Direction::In => Direction::In,
          nusb::transfer::Direction::Out => Direction::Out,
        },
        r#type: match endpoint.transfer_type() {
          nusb::descriptors::TransferType::Control => UsbEndpointType::Control,
          nusb::descriptors::TransferType::Isochronous => {
            UsbEndpointType::Isochronous
          }
          nusb::descriptors::TransferType::Bulk => UsbEndpointType::Bulk,
          nusb::descriptors::TransferType::Interrupt => {
            UsbEndpointType::Interrupt
          }
        },
        packet_size: endpoint.max_packet_size() as u16,
      })
      .collect(),
  }
}

async fn read_string(
  device: &nusb::Device,
  index: NonZeroU8,
) -> Option<String> {
  device
    .get_string_descriptor(index, DEFAULT_LANGUAGE_ID, ENUMERATION_TIMEOUT)
    .await
    .ok()
}

/// Read the WEBUSB_URL from the WebUSB platform capability descriptor.
/// https://wicg.github.io/webusb/#webusb-platform-capability-descriptor
///
/// Not possible on Windows: reading the URL requires a device-level vendor
/// control request, which WinUSB only allows through a claimed interface.
#[cfg(target_os = "windows")]
async fn read_webusb_url(_device: &nusb::Device) -> Option<String> {
  None
}

/// Read the WEBUSB_URL from the WebUSB platform capability descriptor.
/// https://wicg.github.io/webusb/#webusb-platform-capability-descriptor
#[cfg(not(target_os = "windows"))]
async fn read_webusb_url(device: &nusb::Device) -> Option<String> {
  // Read the BOS descriptor header to learn its total length.
  let header = device
    .control_in(
      ControlIn {
        control_type: ControlType::Standard,
        recipient: Recipient::Device,
        request: GET_DESCRIPTOR_REQUEST,
        value: BOS_DESCRIPTOR_TYPE << 8,
        index: 0,
        length: 5,
      },
      ENUMERATION_TIMEOUT,
    )
    .await
    .ok()?;
  if header.len() < 5 {
    return None;
  }

  // Read the full BOS descriptor.
  let total_length = u16::from_le_bytes([header[2], header[3]]);
  let bos = device
    .control_in(
      ControlIn {
        control_type: ControlType::Standard,
        recipient: Recipient::Device,
        request: GET_DESCRIPTOR_REQUEST,
        value: BOS_DESCRIPTOR_TYPE << 8,
        index: 0,
        length: total_length,
      },
      ENUMERATION_TIMEOUT,
    )
    .await
    .ok()?;

  let (vendor_code, landing_page_id) = parse_bos(&bos)?;

  // Read the URL descriptor.
  let url_descriptor = device
    .control_in(
      ControlIn {
        control_type: ControlType::Vendor,
        recipient: Recipient::Device,
        request: vendor_code,
        value: landing_page_id as u16,
        index: GET_URL_REQUEST,
        length: 255,
      },
      ENUMERATION_TIMEOUT,
    )
    .await
    .ok()?;

  parse_webusb_url(&url_descriptor)
}

/// Native connect/disconnect event source.
pub(crate) struct NativeEvents {
  watch: nusb::hotplug::HotplugWatch,
}

impl NativeEvents {
  pub(crate) fn new() -> Result<Self> {
    let watch = nusb::watch_devices().map_err(map_err)?;
    Ok(NativeEvents { watch })
  }

  pub(crate) async fn next(&mut self) -> Option<UsbConnectionEvent> {
    loop {
      match self.watch.next().await? {
        HotplugEvent::Connected(info) => {
          if info.class() == HUB_CLASS {
            continue;
          }
          let device = build_device(info).await;
          return Some(UsbConnectionEvent::Connect(device));
        }
        HotplugEvent::Disconnected(device_id) => {
          let entry = registry().lock().unwrap().remove(&device_id);
          if let Some(entry) = entry {
            return Some(UsbConnectionEvent::Disconnect {
              id: entry.id,
              vendor_id: entry.vendor_id,
              product_id: entry.product_id,
            });
          }
        }
      }
    }
  }
}
