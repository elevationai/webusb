#[cfg(feature = "mock")]
pub(crate) mod mock;
#[cfg(feature = "native")]
pub(crate) mod native;

use crate::Direction;
use crate::Result;
use crate::UsbControlTransferParameters;
use crate::UsbEndpointType;

/// Outcome of a transfer that completed at the protocol level: stalls and
/// babble are reported as statuses per the WebUSB specification, not errors.
pub(crate) enum TransferOutcome<T> {
  Ok(T),
  Stall,
  // Only produced by the mock backend; nusb does not surface babble.
  #[cfg_attr(not(feature = "mock"), allow(dead_code))]
  Babble,
}

pub(crate) enum BackendDevice {
  #[cfg(feature = "native")]
  Native(Box<native::NativeDevice>),
  #[cfg(feature = "mock")]
  Mock(mock::MockDeviceHandle),
}

impl std::fmt::Debug for BackendDevice {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    match self {
      #[cfg(feature = "native")]
      BackendDevice::Native(_) => f.write_str("BackendDevice::Native"),
      #[cfg(feature = "mock")]
      BackendDevice::Mock(_) => f.write_str("BackendDevice::Mock"),
    }
  }
}

macro_rules! dispatch {
  ($self:expr, $d:ident => $e:expr) => {
    match $self {
      #[cfg(feature = "native")]
      BackendDevice::Native($d) => $e,
      #[cfg(feature = "mock")]
      BackendDevice::Mock($d) => $e,
    }
  };
}

impl BackendDevice {
  pub(crate) async fn open(&mut self) -> Result<()> {
    dispatch!(self, d => d.open().await)
  }

  pub(crate) async fn close(&mut self) -> Result<()> {
    dispatch!(self, d => d.close().await)
  }

  pub(crate) async fn select_configuration(
    &mut self,
    configuration_value: u8,
  ) -> Result<()> {
    dispatch!(self, d => d.select_configuration(configuration_value).await)
  }

  pub(crate) async fn claim_interface(
    &mut self,
    interface_number: u8,
  ) -> Result<()> {
    dispatch!(self, d => d.claim_interface(interface_number).await)
  }

  pub(crate) async fn release_interface(
    &mut self,
    interface_number: u8,
  ) -> Result<()> {
    dispatch!(self, d => d.release_interface(interface_number).await)
  }

  pub(crate) async fn select_alternate_interface(
    &mut self,
    interface_number: u8,
    alternate_setting: u8,
  ) -> Result<()> {
    dispatch!(self, d => {
      d.select_alternate_interface(interface_number, alternate_setting).await
    })
  }

  pub(crate) async fn control_transfer_in(
    &mut self,
    setup: &UsbControlTransferParameters,
    length: u16,
  ) -> Result<TransferOutcome<Vec<u8>>> {
    dispatch!(self, d => d.control_transfer_in(setup, length).await)
  }

  pub(crate) async fn control_transfer_out(
    &mut self,
    setup: &UsbControlTransferParameters,
    data: &[u8],
  ) -> Result<TransferOutcome<usize>> {
    dispatch!(self, d => d.control_transfer_out(setup, data).await)
  }

  pub(crate) async fn clear_halt(
    &mut self,
    interface_number: u8,
    endpoint_type: UsbEndpointType,
    direction: Direction,
    endpoint_number: u8,
  ) -> Result<()> {
    dispatch!(self, d => {
      d.clear_halt(interface_number, endpoint_type, direction, endpoint_number)
        .await
    })
  }

  pub(crate) async fn transfer_in(
    &mut self,
    interface_number: u8,
    endpoint_type: UsbEndpointType,
    endpoint_number: u8,
    length: usize,
  ) -> Result<TransferOutcome<Vec<u8>>> {
    dispatch!(self, d => {
      d.transfer_in(interface_number, endpoint_type, endpoint_number, length)
        .await
    })
  }

  pub(crate) async fn transfer_out(
    &mut self,
    interface_number: u8,
    endpoint_type: UsbEndpointType,
    endpoint_number: u8,
    data: &[u8],
  ) -> Result<TransferOutcome<usize>> {
    dispatch!(self, d => {
      d.transfer_out(interface_number, endpoint_type, endpoint_number, data)
        .await
    })
  }

  pub(crate) async fn reset(&mut self) -> Result<()> {
    dispatch!(self, d => d.reset().await)
  }
}
