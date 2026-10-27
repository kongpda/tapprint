//! Inbox USB print devices on Windows (`usbprint.sys`), opened by device path.
//! This is the class driver Windows installs for a USB printer. It is not a
//! vendor driver, and it does not use WinUSB.

use super::UsbDevice;
use crate::escpos;
use crate::model::{Fault, JobError};
use std::mem::{offset_of, size_of};
use windows::Win32::Devices::DeviceAndDriverInstallation::{
    DIGCF_DEVICEINTERFACE, DIGCF_PRESENT, HDEVINFO, SP_DEVICE_INTERFACE_DATA,
    SP_DEVICE_INTERFACE_DETAIL_DATA_W, SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInterfaces,
    SetupDiGetClassDevsW, SetupDiGetDeviceInterfaceDetailW,
};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_IO_PENDING, GENERIC_READ, GENERIC_WRITE, HANDLE, WAIT_TIMEOUT,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_OVERLAPPED, FILE_SHARE_READ, FILE_SHARE_WRITE,
    OPEN_EXISTING, WriteFile,
};
use windows::Win32::System::IO::{
    CancelIoEx, DeviceIoControl, GetOverlappedResult, GetOverlappedResultEx, OVERLAPPED,
};
use windows::Win32::System::Threading::CreateEventW;
use windows::core::{HRESULT, PCWSTR};

/// `{28d78fad-5a12-11d1-ae5b-0000f803a8c2}` — GUID_DEVINTERFACE_USBPRINT.
const USBPRINT: windows::core::GUID =
    windows::core::GUID::from_u128(0x28d78fad_5a12_11d1_ae5b_0000f803a8c2);
/// `IOCTL_USBPRINT_GET_LPT_STATUS` from `usbprint.h`: the port status byte.
const IOCTL_USBPRINT_GET_LPT_STATUS: u32 = 0x0022_0030;
/// `IOCTL_USBPRINT_SOFT_RESET` from `usbprint.h`.
const IOCTL_USBPRINT_SOFT_RESET: u32 = 0x0022_0040;
const WRITE_TIMEOUT_MS: u32 = 20_000;

struct DeviceList(HDEVINFO);

impl Drop for DeviceList {
    fn drop(&mut self) {
        unsafe {
            let _ = SetupDiDestroyDeviceInfoList(self.0);
        }
    }
}

struct File(HANDLE);

impl Drop for File {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

pub fn list() -> Result<Vec<UsbDevice>, String> {
    let paths = device_paths()?;
    let mut found = Vec::new();
    for path in paths {
        let label = vid_pid_label(&path).unwrap_or_else(|| "USB printer".to_string());
        let place = instance_label(&path);
        found.push(UsbDevice {
            id: path,
            label,
            place,
        });
    }
    Ok(found)
}

pub fn write(id: &str, bytes: &[u8], check_paper: bool) -> Result<(), JobError> {
    let fail = |message: String| JobError::new(Fault::Failed, message);
    let file = open_path(id, true)
        .or_else(|_| open_path(id, false))
        .map_err(fail)?;
    let event = Event::new().map_err(fail)?;
    if check_paper && paper_out(&file, &event) {
        return Err(JobError::new(Fault::PaperOut, escpos::PAPER_OUT));
    }
    let mut offset = 0usize;
    while offset < bytes.len() {
        let end = offset.saturating_add(16 * 1024).min(bytes.len());
        let chunk = &bytes[offset..end];
        match write_chunk(&file, &event, chunk) {
            Ok(0) => {
                return Err(failed(
                    &file,
                    &event,
                    "the printer accepted none of the data",
                    offset > 0,
                ));
            }
            Ok(wrote) => offset = offset.saturating_add(wrote),
            Err((message, timed_out)) => {
                return Err(failed(&file, &event, &message, offset > 0 || timed_out));
            }
        }
    }
    Ok(())
}

/// Part of a raster block may be in the printer. Left there, it would swallow
/// the next job's bytes and print garbage, so clear the printer first.
fn failed(file: &File, event: &Event, cause: &str, partial: bool) -> JobError {
    if partial {
        soft_reset(file, event);
    }
    super::failure(cause, partial)
}

/// `false` when the driver does not answer, so a missing status never stops a job.
fn paper_out(file: &File, event: &Event) -> bool {
    // 0 never reads as paper out, so a reply that fills nothing stops no job.
    // Drivers differ in the byte count they report, so any success is read.
    let mut status = [0u8; 1];
    let mut overlapped = OVERLAPPED {
        hEvent: event.0,
        ..OVERLAPPED::default()
    };
    let started = unsafe {
        DeviceIoControl(
            file.0,
            IOCTL_USBPRINT_GET_LPT_STATUS,
            None,
            0,
            Some(status.as_mut_ptr().cast()),
            1,
            None,
            Some(&mut overlapped),
        )
    };
    match started {
        Ok(()) => {}
        Err(error) if error.code() == ERROR_IO_PENDING.to_hresult() => {}
        Err(_) => return false,
    }
    wait(file, &overlapped, 1_000).is_ok() && escpos::port_paper_out(status[0])
}

/// One write that gives up after `WRITE_TIMEOUT_MS`. A plain `WriteFile`
/// waits forever when the printer is out of paper or its cover is open, and
/// that would hold the printer lock and stall every later job.
fn write_chunk(file: &File, event: &Event, chunk: &[u8]) -> Result<usize, (String, bool)> {
    let mut overlapped = OVERLAPPED {
        hEvent: event.0,
        ..OVERLAPPED::default()
    };
    if let Err(error) = unsafe { WriteFile(file.0, Some(chunk), None, Some(&mut overlapped)) }
        && error.code() != ERROR_IO_PENDING.to_hresult()
    {
        return Err((error.message(), false));
    }
    wait(file, &overlapped, WRITE_TIMEOUT_MS).map_err(|timed_out| {
        if timed_out {
            (
                "the printer stopped taking data. Check paper, the cover, and that it is on."
                    .to_string(),
                true,
            )
        } else {
            (
                "the printer was unplugged or refused the data".to_string(),
                false,
            )
        }
    })
}

/// Bytes moved, or `Err(true)` on timeout. On timeout the request is
/// cancelled and waited out, so `overlapped` and the buffer are no longer in
/// use when the caller returns. That last wait has no limit on purpose:
/// returning while the driver may still write to the buffer is unsafe.
fn wait(file: &File, overlapped: &OVERLAPPED, timeout_ms: u32) -> Result<usize, bool> {
    let mut moved = 0u32;
    match unsafe { GetOverlappedResultEx(file.0, overlapped, &mut moved, timeout_ms, false) } {
        Ok(()) => Ok(moved as usize),
        Err(error) if error.code() == HRESULT::from_win32(WAIT_TIMEOUT.0) => {
            unsafe {
                let _ = CancelIoEx(file.0, Some(overlapped));
                let _ = GetOverlappedResult(file.0, overlapped, &mut moved, true);
            }
            Err(true)
        }
        Err(_) => Err(false),
    }
}

fn soft_reset(file: &File, event: &Event) {
    let mut overlapped = OVERLAPPED {
        hEvent: event.0,
        ..OVERLAPPED::default()
    };
    let started = unsafe {
        DeviceIoControl(
            file.0,
            IOCTL_USBPRINT_SOFT_RESET,
            None,
            0,
            None,
            0,
            None,
            Some(&mut overlapped),
        )
    };
    match started {
        Ok(()) => {}
        Err(error) if error.code() == ERROR_IO_PENDING.to_hresult() => {}
        Err(_) => return,
    }
    let _ = wait(file, &overlapped, 2_000);
}

struct Event(HANDLE);

impl Event {
    fn new() -> Result<Self, String> {
        unsafe { CreateEventW(None, true, false, PCWSTR::null()) }
            .map(Self)
            .map_err(|error| error.message())
    }
}

impl Drop for Event {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

fn device_paths() -> Result<Vec<String>, String> {
    let raw = unsafe {
        SetupDiGetClassDevsW(
            Some(&USBPRINT),
            PCWSTR::null(),
            None,
            DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
        )
    }
    .map_err(|error| error.to_string())?;
    let list = DeviceList(raw);
    let mut paths = Vec::new();
    let mut index = 0u32;
    loop {
        let mut interface = SP_DEVICE_INTERFACE_DATA {
            cbSize: u32::try_from(size_of::<SP_DEVICE_INTERFACE_DATA>()).unwrap_or(0),
            ..SP_DEVICE_INTERFACE_DATA::default()
        };
        if unsafe { SetupDiEnumDeviceInterfaces(list.0, None, &USBPRINT, index, &mut interface) }
            .is_err()
        {
            break;
        }
        index = index.saturating_add(1);
        if let Some(path) = interface_path(list.0, &interface) {
            paths.push(path);
        }
    }
    Ok(paths)
}

fn interface_path(list: HDEVINFO, interface: &SP_DEVICE_INTERFACE_DATA) -> Option<String> {
    let mut required = 0u32;
    let _ = unsafe {
        SetupDiGetDeviceInterfaceDetailW(list, interface, None, 0, Some(&mut required), None)
    };
    if required < 8 {
        return None;
    }
    // u32 words, so the buffer has the 4-byte alignment the struct needs.
    // A byte buffer may start at any address.
    let mut words = vec![0u32; (required as usize).div_ceil(4)];
    words[0] = if cfg!(target_pointer_width = "64") {
        8
    } else {
        6
    };
    let detail = words
        .as_mut_ptr()
        .cast::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>();
    unsafe {
        SetupDiGetDeviceInterfaceDetailW(list, interface, Some(detail), required, None, None)
            .ok()?;
    }
    let buffer: Vec<u8> = words.iter().flat_map(|word| word.to_ne_bytes()).collect();
    let start = offset_of!(SP_DEVICE_INTERFACE_DETAIL_DATA_W, DevicePath);
    let rest = buffer.get(start..required as usize)?;
    let mut units = Vec::new();
    let mut index = 0;
    while index + 1 < rest.len() {
        let unit = u16::from_le_bytes([rest[index], rest[index + 1]]);
        if unit == 0 {
            break;
        }
        units.push(unit);
        index += 2;
    }
    String::from_utf16(&units).ok()
}

fn open_path(path: &str, read_write: bool) -> Result<File, String> {
    let mut wide: Vec<u16> = path.encode_utf16().collect();
    wide.push(0);
    let access = if read_write {
        (GENERIC_READ | GENERIC_WRITE).0
    } else {
        GENERIC_WRITE.0
    };
    let handle = unsafe {
        CreateFileW(
            PCWSTR(wide.as_ptr()),
            access,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OVERLAPPED,
            None,
        )
    }
    .map_err(|error| error.to_string())?;
    Ok(File(handle))
}

fn vid_pid_label(path: &str) -> Option<String> {
    let lower = path.to_ascii_lowercase();
    let vid = between(&lower, "vid_", 4)?;
    let pid = between(&lower, "pid_", 4)?;
    Some(format!("USB {vid}:{pid}"))
}

/// The device instance part of `\\?\usb#vid_04b8&pid_0202#<instance>#{guid}`.
/// It is the serial number, or a port-based id when the printer has none.
fn instance_label(path: &str) -> String {
    let instance = path.split('#').nth(2).unwrap_or_default();
    let short: String = instance.chars().take(24).collect();
    if short.is_empty() {
        String::new()
    } else {
        format!("id {short}")
    }
}

fn between(text: &str, marker: &str, len: usize) -> Option<String> {
    let start = text.find(marker)? + marker.len();
    let end = start.checked_add(len)?;
    let slice = text.get(start..end)?;
    if slice.chars().all(|ch| ch.is_ascii_hexdigit()) {
        Some(slice.to_string())
    } else {
        None
    }
}
