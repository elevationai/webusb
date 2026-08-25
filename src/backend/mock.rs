//! In-memory mock backend for tests and hardware-free development.
//!
//! Create a mock [`Usb`][crate::Usb] with [`crate::Usb::mock`], then plug and
//! unplug devices through the returned [`MockController`].

use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::Mutex;

#[cfg(feature = "serde")]
use serde::Deserialize;
#[cfg(feature = "serde")]
use serde::Serialize;

use super::TransferOutcome;
use crate::next_device_id;
use crate::DeviceData;
use crate::Direction;
use crate::Error;
use crate::Result;
use crate::UsbConfiguration;
use crate::UsbConnectionEvent;
use crate::UsbControlTransferParameters;
use crate::UsbDevice;
use crate::UsbEndpointType;
use crate::EP_DIR_IN;
use crate::EP_DIR_OUT;

/// Description of a mock USB device.
///
/// `in_data` maps an endpoint number to the canned payload returned by IN
/// transfers on that endpoint. `stalled_endpoints` lists endpoint addresses
/// (`0x80 | n` for IN, `n` for OUT) that start out halted; a halted endpoint
/// reports `stall` until `clear_halt` is called.
#[derive(Clone, Debug, Default)]
#[cfg_attr(
  feature = "serde",
  derive(Serialize, Deserialize),
  serde(rename_all = "camelCase", default)
)]
pub struct MockDeviceConfig {
  pub vendor_id: u16,
  pub product_id: u16,
  pub device_class: u8,
  pub device_subclass: u8,
  pub device_protocol: u8,
  pub device_version_major: u8,
  pub device_version_minor: u8,
  pub device_version_subminor: u8,
  pub usb_version_major: u8,
  pub usb_version_minor: u8,
  pub usb_version_subminor: u8,
  pub manufacturer_name: Option<String>,
  pub product_name: Option<String>,
  pub serial_number: Option<String>,
  pub configurations: Vec<UsbConfiguration>,
  pub active_configuration: Option<u8>,
  pub url: Option<String>,
  pub in_data: HashMap<u8, Vec<u8>>,
  pub stalled_endpoints: Vec<u8>,
  /// Endpoint addresses whose transfers report `babble`.
  pub babble_endpoints: Vec<u8>,
}

pub(crate) struct MockDeviceState {
  config: MockDeviceConfig,
  connected: bool,
  halted: HashSet<u8>,
  written: Vec<(u8, Vec<u8>)>,
}

type SharedDeviceState = Arc<Mutex<MockDeviceState>>;

#[derive(Default)]
pub(crate) struct MockHub {
  devices: Vec<(u64, SharedDeviceState)>,
  senders: Vec<async_channel::Sender<UsbConnectionEvent>>,
}

#[derive(Clone, Default)]
pub(crate) struct MockHubRef(Arc<Mutex<MockHub>>);

impl MockHubRef {
  pub(crate) fn enumerate(&self) -> Vec<UsbDevice> {
    let hub = self.0.lock().unwrap();
    hub
      .devices
      .iter()
      .filter(|(_, state)| state.lock().unwrap().connected)
      .map(|(id, state)| build_device(*id, state.clone()))
      .collect()
  }

  pub(crate) fn subscribe(
    &self,
  ) -> async_channel::Receiver<UsbConnectionEvent> {
    let (sender, receiver) = async_channel::unbounded();
    self.0.lock().unwrap().senders.push(sender);
    receiver
  }
}

fn build_device(id: u64, state: SharedDeviceState) -> UsbDevice {
  let data = {
    let state = state.lock().unwrap();
    let config = &state.config;
    DeviceData {
      configurations: config.configurations.clone(),
      active_configuration_value: config.active_configuration,
      device_class: config.device_class,
      device_subclass: config.device_subclass,
      device_protocol: config.device_protocol,
      device_version_major: config.device_version_major,
      device_version_minor: config.device_version_minor,
      device_version_subminor: config.device_version_subminor,
      manufacturer_name: config.manufacturer_name.clone(),
      product_id: config.product_id,
      product_name: config.product_name.clone(),
      serial_number: config.serial_number.clone(),
      usb_version_major: config.usb_version_major,
      usb_version_minor: config.usb_version_minor,
      usb_version_subminor: config.usb_version_subminor,
      vendor_id: config.vendor_id,
      url: config.url.clone(),
    }
  };
  UsbDevice::from_parts(
    id,
    data,
    super::BackendDevice::Mock(MockDeviceHandle { state }),
  )
}

/// Plugs and unplugs devices on a mock [`Usb`][crate::Usb], and inspects
/// what was written to them.
pub struct MockController {
  hub: MockHubRef,
}

impl MockController {
  pub(crate) fn new(hub: MockHubRef) -> Self {
    MockController { hub }
  }

  /// Connect a new mock device, firing a connect event. Returns the device id.
  pub fn add_device(&self, config: MockDeviceConfig) -> u64 {
    let id = next_device_id();
    let halted = config.stalled_endpoints.iter().copied().collect();
    let state = Arc::new(Mutex::new(MockDeviceState {
      config,
      connected: true,
      halted,
      written: Vec::new(),
    }));

    let mut hub = self.hub.0.lock().unwrap();
    hub.devices.push((id, state.clone()));
    hub.senders.retain(|sender| {
      sender
        .try_send(UsbConnectionEvent::Connect(build_device(id, state.clone())))
        .is_ok()
    });
    id
  }

  /// Disconnect a mock device, firing a disconnect event. Pending handles
  /// observe [`Error::Disconnected`].
  pub fn remove_device(&self, id: u64) -> Result<()> {
    let mut hub = self.hub.0.lock().unwrap();
    let index = hub
      .devices
      .iter()
      .position(|(device_id, _)| *device_id == id)
      .ok_or(Error::NotFound)?;
    let (_, state) = hub.devices.remove(index);
    let (vendor_id, product_id) = {
      let mut state = state.lock().unwrap();
      state.connected = false;
      (state.config.vendor_id, state.config.product_id)
    };
    hub.senders.retain(|sender| {
      sender
        .try_send(UsbConnectionEvent::Disconnect {
          id,
          vendor_id,
          product_id,
        })
        .is_ok()
    });
    Ok(())
  }

  /// Everything written to the device so far, as (endpoint number, data)
  /// pairs. Control transfers are recorded as endpoint 0.
  pub fn written(&self, id: u64) -> Result<Vec<(u8, Vec<u8>)>> {
    let state = self.device_state(id)?;
    let state = state.lock().unwrap();
    Ok(state.written.clone())
  }

  /// Halt an endpoint (`0x80 | n` addresses an IN endpoint) so subsequent
  /// transfers report `stall` until the endpoint is cleared.
  pub fn halt_endpoint(&self, id: u64, address: u8) -> Result<()> {
    let state = self.device_state(id)?;
    state.lock().unwrap().halted.insert(address);
    Ok(())
  }

  fn device_state(&self, id: u64) -> Result<SharedDeviceState> {
    let hub = self.hub.0.lock().unwrap();
    hub
      .devices
      .iter()
      .find(|(device_id, _)| *device_id == id)
      .map(|(_, state)| state.clone())
      .ok_or(Error::NotFound)
  }
}

pub(crate) struct MockDeviceHandle {
  state: SharedDeviceState,
}

impl MockDeviceHandle {
  fn check_connected(&self) -> Result<()> {
    if self.state.lock().unwrap().connected {
      Ok(())
    } else {
      Err(Error::Disconnected)
    }
  }

  pub(crate) async fn open(&mut self) -> Result<()> {
    self.check_connected()
  }

  pub(crate) async fn close(&mut self) -> Result<()> {
    Ok(())
  }

  pub(crate) async fn select_configuration(
    &mut self,
    _configuration_value: u8,
  ) -> Result<()> {
    self.check_connected()
  }

  pub(crate) async fn claim_interface(
    &mut self,
    _interface_number: u8,
  ) -> Result<()> {
    self.check_connected()
  }

  pub(crate) async fn release_interface(
    &mut self,
    _interface_number: u8,
  ) -> Result<()> {
    self.check_connected()
  }

  pub(crate) async fn select_alternate_interface(
    &mut self,
    _interface_number: u8,
    _alternate_setting: u8,
  ) -> Result<()> {
    self.check_connected()
  }

  pub(crate) async fn control_transfer_in(
    &mut self,
    setup: &UsbControlTransferParameters,
    length: u16,
  ) -> Result<TransferOutcome<Vec<u8>>> {
    self.check_connected()?;
    // Echo the setup packet, cycled to the requested length, so tests can
    // verify the parameters made it through.
    let pattern = [
      setup.request,
      setup.value.to_le_bytes()[0],
      setup.value.to_le_bytes()[1],
      setup.index.to_le_bytes()[0],
      setup.index.to_le_bytes()[1],
    ];
    let data = pattern
      .iter()
      .copied()
      .cycle()
      .take(length as usize)
      .collect();
    Ok(TransferOutcome::Ok(data))
  }

  pub(crate) async fn control_transfer_out(
    &mut self,
    _setup: &UsbControlTransferParameters,
    data: &[u8],
  ) -> Result<TransferOutcome<usize>> {
    self.check_connected()?;
    let mut state = self.state.lock().unwrap();
    state.written.push((0, data.to_vec()));
    Ok(TransferOutcome::Ok(data.len()))
  }

  pub(crate) async fn clear_halt(
    &mut self,
    _interface_number: u8,
    _endpoint_type: UsbEndpointType,
    direction: Direction,
    endpoint_number: u8,
  ) -> Result<()> {
    self.check_connected()?;
    let address = address(direction, endpoint_number);
    self.state.lock().unwrap().halted.remove(&address);
    Ok(())
  }

  pub(crate) async fn transfer_in(
    &mut self,
    _interface_number: u8,
    _endpoint_type: UsbEndpointType,
    endpoint_number: u8,
    length: usize,
  ) -> Result<TransferOutcome<Vec<u8>>> {
    self.check_connected()?;
    let state = self.state.lock().unwrap();
    if state.halted.contains(&(EP_DIR_IN | endpoint_number)) {
      return Ok(TransferOutcome::Stall);
    }
    if state
      .config
      .babble_endpoints
      .contains(&(EP_DIR_IN | endpoint_number))
    {
      return Ok(TransferOutcome::Babble);
    }
    let mut data = state
      .config
      .in_data
      .get(&endpoint_number)
      .cloned()
      .unwrap_or_default();
    data.truncate(length);
    Ok(TransferOutcome::Ok(data))
  }

  pub(crate) async fn transfer_out(
    &mut self,
    _interface_number: u8,
    _endpoint_type: UsbEndpointType,
    endpoint_number: u8,
    data: &[u8],
  ) -> Result<TransferOutcome<usize>> {
    self.check_connected()?;
    let mut state = self.state.lock().unwrap();
    if state.halted.contains(&(EP_DIR_OUT | endpoint_number)) {
      return Ok(TransferOutcome::Stall);
    }
    if state
      .config
      .babble_endpoints
      .contains(&(EP_DIR_OUT | endpoint_number))
    {
      return Ok(TransferOutcome::Babble);
    }
    state.written.push((endpoint_number, data.to_vec()));
    Ok(TransferOutcome::Ok(data.len()))
  }

  pub(crate) async fn reset(&mut self) -> Result<()> {
    self.check_connected()
  }
}

fn address(direction: Direction, endpoint_number: u8) -> u8 {
  match direction {
    Direction::In => EP_DIR_IN | endpoint_number,
    Direction::Out => EP_DIR_OUT | endpoint_number,
  }
}
