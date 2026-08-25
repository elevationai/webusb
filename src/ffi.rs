//! C ABI consumed by the Deno bindings in `mod.ts`.
//!
//! All functions are synchronous; callers are expected to invoke the
//! potentially-blocking ones with Deno FFI's `nonblocking: true`, which runs
//! them on a thread pool and returns a Promise.
//!
//! Structured data crosses the boundary as JSON strings allocated by
//! [`webusb_free_string`]-compatible `CString`s; transfer payloads cross as
//! raw caller-owned buffers.

use std::collections::HashMap;
use std::ffi::c_char;
use std::ffi::CStr;
use std::ffi::CString;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;

use futures_lite::future::block_on;

use crate::Direction;
use crate::Error;
use crate::MockController;
use crate::MockDeviceConfig;
use crate::Result;
use crate::Usb;
use crate::UsbConnectionEvent;
use crate::UsbControlTransferParameters;
use crate::UsbDevice;
use crate::UsbDeviceFilter;
use crate::UsbRecipient;
use crate::UsbRequestType;
use crate::UsbTransferStatus;

const OK: i32 = 0;
const ERR_NOT_FOUND: i32 = -1;
const ERR_INVALID_STATE: i32 = -2;
const ERR_INVALID_ACCESS: i32 = -3;
const ERR_NOT_SUPPORTED: i32 = -4;
const ERR_DISCONNECTED: i32 = -5;
const ERR_BUSY: i32 = -6;
const ERR_ACCESS: i32 = -7;
const ERR_IO: i32 = -8;
const ERR_NOT_INITIALIZED: i32 = -9;
const ERR_INVALID_INPUT: i32 = -10;

const STATUS_OK: u8 = 0;
const STATUS_STALL: u8 = 1;
const STATUS_BABBLE: u8 = 2;

fn error_code(error: &Error) -> i32 {
  match error {
    Error::NotFound => ERR_NOT_FOUND,
    Error::InvalidState => ERR_INVALID_STATE,
    Error::InvalidAccess => ERR_INVALID_ACCESS,
    Error::NotSupported => ERR_NOT_SUPPORTED,
    Error::Disconnected => ERR_DISCONNECTED,
    Error::Busy => ERR_BUSY,
    Error::Access => ERR_ACCESS,
    Error::Io(_) => ERR_IO,
  }
}

fn status_code(status: UsbTransferStatus) -> u8 {
  match status {
    UsbTransferStatus::Ok => STATUS_OK,
    UsbTransferStatus::Stall => STATUS_STALL,
    UsbTransferStatus::Babble => STATUS_BABBLE,
  }
}

struct FfiState {
  usb: Usb,
  controller: Option<MockController>,
  devices: HashMap<u64, Arc<Mutex<UsbDevice>>>,
  events: Option<async_channel::Receiver<String>>,
}

fn state() -> &'static Mutex<Option<FfiState>> {
  static STATE: OnceLock<Mutex<Option<FfiState>>> = OnceLock::new();
  STATE.get_or_init(|| Mutex::new(None))
}

fn json_string(value: &impl serde::Serialize) -> *mut c_char {
  match serde_json::to_string(value) {
    Ok(json) => match CString::new(json) {
      Ok(cstring) => cstring.into_raw(),
      Err(_) => std::ptr::null_mut(),
    },
    Err(_) => std::ptr::null_mut(),
  }
}

fn json_ok(value: &impl serde::Serialize) -> *mut c_char {
  json_string(&serde_json::json!({ "ok": value }))
}

fn json_err(code: i32) -> *mut c_char {
  json_string(&serde_json::json!({ "err": code }))
}

/// Track a device, keeping any existing (possibly open) handle for the id.
fn track_device(
  devices: &mut HashMap<u64, Arc<Mutex<UsbDevice>>>,
  device: UsbDevice,
) -> Arc<Mutex<UsbDevice>> {
  devices
    .entry(device.id)
    .or_insert_with(|| Arc::new(Mutex::new(device)))
    .clone()
}

fn with_device<R>(
  id: u64,
  f: impl FnOnce(&mut UsbDevice) -> Result<R>,
) -> Result<R> {
  let device = {
    let state = state().lock().unwrap();
    let state = state.as_ref().ok_or(Error::InvalidState)?;
    state.devices.get(&id).cloned().ok_or(Error::NotFound)?
  };
  let mut device = device.lock().unwrap();
  f(&mut device)
}

fn device_op(id: u64, f: impl FnOnce(&mut UsbDevice) -> Result<()>) -> i32 {
  match with_device(id, f) {
    Ok(()) => OK,
    Err(error) => error_code(&error),
  }
}

/// Initialize the library. `use_mock != 0` selects the in-memory mock
/// backend; otherwise real devices are used.
#[no_mangle]
pub extern "C" fn webusb_init(use_mock: u8) -> i32 {
  let (usb, controller) = if use_mock != 0 {
    let (usb, controller) = Usb::mock();
    (usb, Some(controller))
  } else {
    match Usb::new() {
      Ok(usb) => (usb, None),
      Err(error) => return error_code(&error),
    }
  };

  *state().lock().unwrap() = Some(FfiState {
    usb,
    controller,
    devices: HashMap::new(),
    events: None,
  });
  OK
}

/// Free a string returned by any `webusb_*` function.
///
/// # Safety
/// `ptr` must be a pointer previously returned by this library and not yet
/// freed. Passing null is allowed and does nothing.
#[no_mangle]
pub unsafe extern "C" fn webusb_free_string(ptr: *mut c_char) {
  if !ptr.is_null() {
    drop(CString::from_raw(ptr));
  }
}

/// Enumerate devices. Returns `{"ok": [device, ...]}` or `{"err": code}`.
#[no_mangle]
pub extern "C" fn webusb_get_devices() -> *mut c_char {
  let mut state = state().lock().unwrap();
  let state = match state.as_mut() {
    Some(state) => state,
    None => return json_err(ERR_NOT_INITIALIZED),
  };

  let devices = match block_on(state.usb.devices()) {
    Ok(devices) => devices,
    Err(error) => return json_err(error_code(&error)),
  };

  let tracked: Vec<Arc<Mutex<UsbDevice>>> = devices
    .into_iter()
    .map(|device| track_device(&mut state.devices, device))
    .collect();
  let snapshots: Vec<serde_json::Value> = tracked
    .iter()
    .map(|device| {
      let device = device.lock().unwrap();
      serde_json::to_value(&*device).unwrap_or(serde_json::Value::Null)
    })
    .collect();
  json_ok(&snapshots)
}

/// Return the first device matching the JSON-encoded filter list.
/// Returns `{"ok": device}` or `{"err": code}`.
///
/// # Safety
/// `filters_json` must be a valid NUL-terminated UTF-8 string.
#[no_mangle]
pub unsafe extern "C" fn webusb_request_device(
  filters_json: *const c_char,
) -> *mut c_char {
  if filters_json.is_null() {
    return json_err(ERR_INVALID_INPUT);
  }
  let filters = match CStr::from_ptr(filters_json).to_str() {
    Ok(json) => match serde_json::from_str::<Vec<UsbDeviceFilter>>(json) {
      Ok(filters) => filters,
      Err(_) => return json_err(ERR_INVALID_INPUT),
    },
    Err(_) => return json_err(ERR_INVALID_INPUT),
  };

  let mut state = state().lock().unwrap();
  let state = match state.as_mut() {
    Some(state) => state,
    None => return json_err(ERR_NOT_INITIALIZED),
  };

  let device = match block_on(state.usb.request_device(&filters)) {
    Ok(device) => device,
    Err(error) => return json_err(error_code(&error)),
  };

  let tracked = track_device(&mut state.devices, device);
  let device = tracked.lock().unwrap();
  json_ok(&*device)
}

#[no_mangle]
pub extern "C" fn webusb_open(id: u64) -> i32 {
  device_op(id, |device| block_on(device.open()))
}

#[no_mangle]
pub extern "C" fn webusb_close(id: u64) -> i32 {
  device_op(id, |device| block_on(device.close()))
}

#[no_mangle]
pub extern "C" fn webusb_reset(id: u64) -> i32 {
  device_op(id, |device| block_on(device.reset()))
}

#[no_mangle]
pub extern "C" fn webusb_select_configuration(
  id: u64,
  configuration_value: u8,
) -> i32 {
  device_op(id, |device| {
    block_on(device.select_configuration(configuration_value))
  })
}

#[no_mangle]
pub extern "C" fn webusb_claim_interface(id: u64, interface_number: u8) -> i32 {
  device_op(id, |device| {
    block_on(device.claim_interface(interface_number))
  })
}

#[no_mangle]
pub extern "C" fn webusb_release_interface(
  id: u64,
  interface_number: u8,
) -> i32 {
  device_op(id, |device| {
    block_on(device.release_interface(interface_number))
  })
}

#[no_mangle]
pub extern "C" fn webusb_select_alternate_interface(
  id: u64,
  interface_number: u8,
  alternate_setting: u8,
) -> i32 {
  device_op(id, |device| {
    block_on(
      device.select_alternate_interface(interface_number, alternate_setting),
    )
  })
}

/// `direction`: 0 = out, 1 = in.
#[no_mangle]
pub extern "C" fn webusb_clear_halt(
  id: u64,
  direction: u8,
  endpoint_number: u8,
) -> i32 {
  let direction = if direction == 0 {
    Direction::Out
  } else {
    Direction::In
  };
  device_op(id, |device| {
    block_on(device.clear_halt(direction, endpoint_number))
  })
}

fn request_type_from(raw: u8) -> Result<UsbRequestType> {
  match raw {
    0 => Ok(UsbRequestType::Standard),
    1 => Ok(UsbRequestType::Class),
    2 => Ok(UsbRequestType::Vendor),
    _ => Err(Error::NotSupported),
  }
}

fn recipient_from(raw: u8) -> Result<UsbRecipient> {
  match raw {
    0 => Ok(UsbRecipient::Device),
    1 => Ok(UsbRecipient::Interface),
    2 => Ok(UsbRecipient::Endpoint),
    3 => Ok(UsbRecipient::Other),
    _ => Err(Error::NotSupported),
  }
}

/// Perform an IN transfer, writing up to `buf_len` bytes into `buf`.
/// Returns the number of bytes received, or a negative error code.
/// `out_status` receives 0 (ok), 1 (stall) or 2 (babble).
///
/// # Safety
/// `buf` must be valid for writes of `buf_len` bytes and `out_status` for
/// one byte, for the duration of the call.
#[no_mangle]
pub unsafe extern "C" fn webusb_transfer_in(
  id: u64,
  endpoint_number: u8,
  buf: *mut u8,
  buf_len: u32,
  out_status: *mut u8,
) -> i64 {
  if buf.is_null() || out_status.is_null() {
    return ERR_INVALID_INPUT as i64;
  }
  match with_device(id, |device| {
    block_on(device.transfer_in(endpoint_number, buf_len as usize))
  }) {
    Ok(result) => {
      let len = result.data.len().min(buf_len as usize);
      std::ptr::copy_nonoverlapping(result.data.as_ptr(), buf, len);
      *out_status = status_code(result.status);
      len as i64
    }
    Err(error) => error_code(&error) as i64,
  }
}

/// Perform an OUT transfer. Returns bytes written or a negative error code.
///
/// # Safety
/// `data` must be valid for reads of `data_len` bytes and `out_status` for
/// one byte of writes, for the duration of the call.
#[no_mangle]
pub unsafe extern "C" fn webusb_transfer_out(
  id: u64,
  endpoint_number: u8,
  data: *const u8,
  data_len: u32,
  out_status: *mut u8,
) -> i64 {
  if (data.is_null() && data_len != 0) || out_status.is_null() {
    return ERR_INVALID_INPUT as i64;
  }
  let payload = if data_len == 0 {
    &[][..]
  } else {
    std::slice::from_raw_parts(data, data_len as usize)
  };
  match with_device(id, |device| {
    block_on(device.transfer_out(endpoint_number, payload))
  }) {
    Ok(result) => {
      *out_status = status_code(result.status);
      result.bytes_written as i64
    }
    Err(error) => error_code(&error) as i64,
  }
}

/// Perform a control IN transfer. See [`webusb_transfer_in`] for the
/// return value and status conventions.
///
/// # Safety
/// `buf` must be valid for writes of `length` bytes and `out_status` for one
/// byte, for the duration of the call.
#[no_mangle]
pub unsafe extern "C" fn webusb_control_transfer_in(
  id: u64,
  request_type: u8,
  recipient: u8,
  request: u8,
  value: u16,
  index: u16,
  buf: *mut u8,
  length: u16,
  out_status: *mut u8,
) -> i64 {
  if buf.is_null() || out_status.is_null() {
    return ERR_INVALID_INPUT as i64;
  }
  let setup = match setup_from(request_type, recipient, request, value, index) {
    Ok(setup) => setup,
    Err(error) => return error_code(&error) as i64,
  };
  match with_device(id, |device| {
    block_on(device.control_transfer_in(setup, length))
  }) {
    Ok(result) => {
      let len = result.data.len().min(length as usize);
      std::ptr::copy_nonoverlapping(result.data.as_ptr(), buf, len);
      *out_status = status_code(result.status);
      len as i64
    }
    Err(error) => error_code(&error) as i64,
  }
}

/// Perform a control OUT transfer. See [`webusb_transfer_out`] for the
/// return value and status conventions.
///
/// # Safety
/// `data` must be valid for reads of `data_len` bytes and `out_status` for
/// one byte of writes, for the duration of the call.
#[no_mangle]
pub unsafe extern "C" fn webusb_control_transfer_out(
  id: u64,
  request_type: u8,
  recipient: u8,
  request: u8,
  value: u16,
  index: u16,
  data: *const u8,
  data_len: u32,
  out_status: *mut u8,
) -> i64 {
  if (data.is_null() && data_len != 0) || out_status.is_null() {
    return ERR_INVALID_INPUT as i64;
  }
  let setup = match setup_from(request_type, recipient, request, value, index) {
    Ok(setup) => setup,
    Err(error) => return error_code(&error) as i64,
  };
  let payload = if data_len == 0 {
    &[][..]
  } else {
    std::slice::from_raw_parts(data, data_len as usize)
  };
  match with_device(id, |device| {
    block_on(device.control_transfer_out(setup, payload))
  }) {
    Ok(result) => {
      *out_status = status_code(result.status);
      result.bytes_written as i64
    }
    Err(error) => error_code(&error) as i64,
  }
}

fn setup_from(
  request_type: u8,
  recipient: u8,
  request: u8,
  value: u16,
  index: u16,
) -> Result<UsbControlTransferParameters> {
  Ok(UsbControlTransferParameters {
    request_type: request_type_from(request_type)?,
    recipient: recipient_from(recipient)?,
    request,
    value,
    index,
  })
}

/// Isochronous IN transfer. Parameters are validated; the transfer itself is
/// currently unsupported and reports `ERR_NOT_SUPPORTED`.
#[no_mangle]
pub extern "C" fn webusb_isochronous_transfer_in(
  id: u64,
  endpoint_number: u8,
) -> i32 {
  device_op(id, |device| {
    block_on(device.isochronous_transfer_in(endpoint_number, &[])).map(|_| ())
  })
}

/// Isochronous OUT transfer. Parameters are validated; the transfer itself
/// is currently unsupported and reports `ERR_NOT_SUPPORTED`.
///
/// # Safety
/// `data` must be valid for reads of `data_len` bytes for the duration of
/// the call, or null when `data_len` is 0.
#[no_mangle]
pub unsafe extern "C" fn webusb_isochronous_transfer_out(
  id: u64,
  endpoint_number: u8,
  data: *const u8,
  data_len: u32,
) -> i32 {
  if data.is_null() && data_len != 0 {
    return ERR_INVALID_INPUT;
  }
  let payload = if data_len == 0 {
    &[][..]
  } else {
    std::slice::from_raw_parts(data, data_len as usize)
  };
  device_op(id, |device| {
    block_on(device.isochronous_transfer_out(endpoint_number, payload, &[]))
      .map(|_| ())
  })
}

/// Start delivering connect/disconnect events to [`webusb_next_event`].
#[no_mangle]
pub extern "C" fn webusb_events_start() -> i32 {
  let mut guard = state().lock().unwrap();
  let ffi_state = match guard.as_mut() {
    Some(ffi_state) => ffi_state,
    None => return ERR_NOT_INITIALIZED,
  };
  if ffi_state.events.is_some() {
    return OK;
  }

  let mut events = match ffi_state.usb.events() {
    Ok(events) => events,
    Err(error) => return error_code(&error),
  };
  let (sender, receiver) = async_channel::unbounded::<String>();
  ffi_state.events = Some(receiver);
  drop(guard);

  std::thread::spawn(move || {
    while let Some(event) = events.next_blocking() {
      let json = match event {
        UsbConnectionEvent::Connect(device) => {
          let mut guard = state().lock().unwrap();
          let Some(ffi_state) = guard.as_mut() else {
            break;
          };
          let tracked = track_device(&mut ffi_state.devices, device);
          let device = tracked.lock().unwrap();
          serde_json::json!({ "event": "connect", "device": &*device })
            .to_string()
        }
        UsbConnectionEvent::Disconnect {
          id,
          vendor_id,
          product_id,
        } => {
          let mut guard = state().lock().unwrap();
          let Some(ffi_state) = guard.as_mut() else {
            break;
          };
          ffi_state.devices.remove(&id);
          serde_json::json!({
            "event": "disconnect",
            "id": id,
            "vendorId": vendor_id,
            "productId": product_id,
          })
          .to_string()
        }
      };
      if sender.send_blocking(json).is_err() {
        break;
      }
    }
  });
  OK
}

/// Block until the next connect/disconnect event. Returns a JSON string, or
/// null once the event stream has been stopped.
#[no_mangle]
pub extern "C" fn webusb_next_event() -> *mut c_char {
  let receiver = {
    let guard = state().lock().unwrap();
    match guard.as_ref().and_then(|s| s.events.clone()) {
      Some(receiver) => receiver,
      None => return std::ptr::null_mut(),
    }
  };
  match receiver.recv_blocking() {
    Ok(json) => match CString::new(json) {
      Ok(cstring) => cstring.into_raw(),
      Err(_) => std::ptr::null_mut(),
    },
    Err(_) => std::ptr::null_mut(),
  }
}

/// Stop event delivery; pending and future [`webusb_next_event`] calls
/// return null.
#[no_mangle]
pub extern "C" fn webusb_events_stop() -> i32 {
  let mut guard = state().lock().unwrap();
  if let Some(ffi_state) = guard.as_mut() {
    if let Some(receiver) = ffi_state.events.take() {
      receiver.close();
    }
  }
  OK
}

/// Connect a mock device described by JSON (`MockDeviceConfig`).
/// Returns the new device id, or a negative error code.
///
/// # Safety
/// `config_json` must be a valid NUL-terminated UTF-8 string.
#[no_mangle]
pub unsafe extern "C" fn webusb_mock_add_device(
  config_json: *const c_char,
) -> i64 {
  if config_json.is_null() {
    return ERR_INVALID_INPUT as i64;
  }
  let config = match CStr::from_ptr(config_json).to_str() {
    Ok(json) => match serde_json::from_str::<MockDeviceConfig>(json) {
      Ok(config) => config,
      Err(_) => return ERR_INVALID_INPUT as i64,
    },
    Err(_) => return ERR_INVALID_INPUT as i64,
  };

  let guard = state().lock().unwrap();
  match guard.as_ref().and_then(|s| s.controller.as_ref()) {
    Some(controller) => controller.add_device(config) as i64,
    None => ERR_NOT_INITIALIZED as i64,
  }
}

/// Disconnect a mock device.
#[no_mangle]
pub extern "C" fn webusb_mock_remove_device(id: u64) -> i32 {
  let guard = state().lock().unwrap();
  match guard.as_ref().and_then(|s| s.controller.as_ref()) {
    Some(controller) => match controller.remove_device(id) {
      Ok(()) => OK,
      Err(error) => error_code(&error),
    },
    None => ERR_NOT_INITIALIZED,
  }
}

/// Everything written to a mock device, as JSON `[[endpoint, [byte, ...]], ...]`.
/// Returns `{"ok": ...}` or `{"err": code}`.
#[no_mangle]
pub extern "C" fn webusb_mock_written(id: u64) -> *mut c_char {
  let guard = state().lock().unwrap();
  match guard.as_ref().and_then(|s| s.controller.as_ref()) {
    Some(controller) => match controller.written(id) {
      Ok(written) => json_ok(&written),
      Err(error) => json_err(error_code(&error)),
    },
    None => json_err(ERR_NOT_INITIALIZED),
  }
}

/// Halt a mock endpoint (`0x80 | n` addresses an IN endpoint).
#[no_mangle]
pub extern "C" fn webusb_mock_halt_endpoint(id: u64, address: u8) -> i32 {
  let guard = state().lock().unwrap();
  match guard.as_ref().and_then(|s| s.controller.as_ref()) {
    Some(controller) => match controller.halt_endpoint(id, address) {
      Ok(()) => OK,
      Err(error) => error_code(&error),
    },
    None => ERR_NOT_INITIALIZED,
  }
}
