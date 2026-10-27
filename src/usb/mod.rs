//! USB printer-class devices.
//!
//! macOS and Linux go through libusb and only claim interface class 7
//! (the standard printer class). Windows uses the inbox USB print device,
//! which is already bound to that class, so no vendor driver is installed.

#[cfg(target_os = "windows")]
mod windows;

use crate::model::{Fault, JobError};

const PARTIAL_NOTE: &str = "Part of the receipt may have printed.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsbDevice {
    pub id: String,
    pub label: String,
    /// Where the printer is plugged in. Shown only when two printers share a label.
    pub place: String,
}

pub fn list() -> Result<Vec<UsbDevice>, String> {
    #[cfg(target_os = "windows")]
    let mut found = windows::list()?;
    #[cfg(not(target_os = "windows"))]
    let mut found = libusb::list()?;
    tell_apart(&mut found);
    Ok(found)
}

/// Two printers of the same model have the same label. Add where each one is
/// plugged in, so the cashier can pick the right one.
fn tell_apart(devices: &mut [UsbDevice]) {
    devices.sort_by(|a, b| a.label.cmp(&b.label).then_with(|| a.place.cmp(&b.place)));
    let labels: Vec<String> = devices.iter().map(|device| device.label.clone()).collect();
    for device in devices.iter_mut() {
        let twins = labels
            .iter()
            .filter(|label| **label == device.label)
            .count();
        if twins > 1 && !device.place.is_empty() {
            device.label = format!("{} · {}", device.label, device.place);
        }
    }
}

/// Send one job. With `check_paper`, ask the printer first and send nothing
/// when it says the paper is out.
pub fn write(id: &str, bytes: &[u8], check_paper: bool) -> Result<(), JobError> {
    #[cfg(target_os = "windows")]
    {
        windows::write(id, bytes, check_paper)
    }
    #[cfg(not(target_os = "windows"))]
    {
        libusb::write(id, bytes, check_paper)
    }
}

/// `vvvv:pppp` from a saved id: `04b8:0202:1-4` on macOS and Linux, or a
/// Windows device path holding `vid_04b8&pid_0202`.
pub fn model_of(id: &str) -> Option<String> {
    let lower = id.to_ascii_lowercase();
    let hex4 = |text: &str| text.len() == 4 && text.chars().all(|ch| ch.is_ascii_hexdigit());
    let after = |marker: &str| {
        let start = lower.find(marker)? + marker.len();
        let value = lower.get(start..start + 4)?;
        hex4(value).then(|| value.to_string())
    };
    if let (Some(vendor), Some(product)) = (after("vid_"), after("pid_")) {
        return Some(format!("{vendor}:{product}"));
    }
    let mut parts = lower.split(':');
    let (vendor, product) = (parts.next()?, parts.next()?);
    (hex4(vendor) && hex4(product)).then(|| format!("{vendor}:{product}"))
}

/// A failed write, with the partial-print note when some bytes went out.
fn failure(cause: &str, partial: bool) -> JobError {
    if !partial {
        return JobError::new(Fault::Failed, cause);
    }
    let stop = if cause.ends_with('.') { "" } else { "." };
    JobError::new(Fault::Partial, format!("{cause}{stop} {PARTIAL_NOTE}"))
}

#[cfg(not(target_os = "windows"))]
mod libusb {
    use super::UsbDevice;
    use crate::escpos;
    use crate::model::{Fault, JobError};
    use rusb::UsbContext;
    use std::sync::OnceLock;
    use std::time::Duration;

    const PRINTER_CLASS: u8 = 0x07;
    /// Printer-class GET_PORT_STATUS: one byte, the old parallel-port status lines.
    const GET_PORT_STATUS: u8 = 1;
    /// Printer-class SOFT_RESET: drop whatever the printer has buffered.
    const SOFT_RESET: u8 = 2;
    const REMOVE_QUEUE_HINT: &str = "If the system printer list already has this device, remove that queue and plug it in again.";

    /// The scan runs every second. One context for the whole run, instead of
    /// starting and stopping libusb each time.
    fn context() -> Result<&'static rusb::Context, String> {
        static CONTEXT: OnceLock<rusb::Context> = OnceLock::new();
        if let Some(context) = CONTEXT.get() {
            return Ok(context);
        }
        let fresh = rusb::Context::new().map_err(|error| error.to_string())?;
        Ok(CONTEXT.get_or_init(|| fresh))
    }

    pub fn list() -> Result<Vec<UsbDevice>, String> {
        let devices = context()?.devices().map_err(|error| error.to_string())?;
        let mut found = Vec::new();
        for device in devices.iter() {
            let device_desc = match device.device_descriptor() {
                Ok(desc) => desc,
                Err(_) => continue,
            };
            let config = match device.config_descriptor(0) {
                Ok(config) => config,
                Err(_) => continue,
            };
            let mut is_printer = false;
            for interface in config.interfaces() {
                for alt in interface.descriptors() {
                    if alt.class_code() == PRINTER_CLASS
                        && alt.endpoint_descriptors().any(|endpoint| {
                            endpoint.direction() == rusb::Direction::Out
                                && endpoint.transfer_type() == rusb::TransferType::Bulk
                        })
                    {
                        is_printer = true;
                    }
                }
            }
            if !is_printer {
                continue;
            }
            let vendor = device_desc.vendor_id();
            let product = device_desc.product_id();
            found.push(UsbDevice {
                id: device_id(&device, &device_desc),
                label: format!("USB {vendor:04x}:{product:04x}"),
                place: format!("port {}", port_path(&device)),
            });
        }
        Ok(found)
    }

    pub fn write(id: &str, bytes: &[u8], check_paper: bool) -> Result<(), JobError> {
        let fail = |message: String| JobError::new(Fault::Failed, message);
        let devices = context()
            .map_err(fail)?
            .devices()
            .map_err(|error| fail(error.to_string()))?;
        for device in devices.iter() {
            let Ok(device_desc) = device.device_descriptor() else {
                continue;
            };
            if device_id(&device, &device_desc) != id {
                continue;
            }
            return write_device(&device, bytes, check_paper);
        }
        Err(fail("that USB printer is no longer connected".to_string()))
    }

    /// Vendor, product, and the physical port. The bus address changes on
    /// every replug. The port does not, so the saved choice survives a replug.
    fn device_id(device: &rusb::Device<rusb::Context>, desc: &rusb::DeviceDescriptor) -> String {
        format!(
            "{:04x}:{:04x}:{}",
            desc.vendor_id(),
            desc.product_id(),
            port_path(device)
        )
    }

    /// `1-2.3`: bus 1, hub port 2, then port 3. Falls back to the bus address.
    fn port_path(device: &rusb::Device<rusb::Context>) -> String {
        match device.port_numbers() {
            Ok(ports) if !ports.is_empty() => {
                let ports: Vec<String> = ports.iter().map(u8::to_string).collect();
                format!("{}-{}", device.bus_number(), ports.join("."))
            }
            _ => format!("{}@{}", device.bus_number(), device.address()),
        }
    }

    fn write_device(
        device: &rusb::Device<rusb::Context>,
        bytes: &[u8],
        check_paper: bool,
    ) -> Result<(), JobError> {
        let fail = |message: String| JobError::new(Fault::Failed, message);
        let config = device
            .config_descriptor(0)
            .map_err(|error| fail(error.to_string()))?;
        let (interface_number, setting, endpoint) = printer_endpoint(&config)
            .ok_or_else(|| fail("the printer has no bulk-out endpoint".to_string()))?;

        let handle = device
            .open()
            .map_err(|error| fail(format!("{error}. {REMOVE_QUEUE_HINT}")))?;

        // Declared after `handle` so this drops first and puts the kernel
        // driver back even when the claim or the write fails.
        #[cfg(target_os = "linux")]
        let _reattach = detach_for_print(&handle, interface_number).map_err(fail)?;

        if handle.claim_interface(interface_number).is_err() {
            let _ = handle.set_active_configuration(config.number());
            handle
                .claim_interface(interface_number)
                .map_err(|error| fail(format!("{error}. {REMOVE_QUEUE_HINT}")))?;
        }
        // Some printers put the printer class on a second alternate setting.
        if setting != 0 {
            handle
                .set_alternate_setting(interface_number, setting)
                .map_err(|error| fail(error.to_string()))?;
        }
        if check_paper && paper_out(&handle, interface_number) {
            return Err(JobError::new(Fault::PaperOut, escpos::PAPER_OUT));
        }

        let result = write_all(&handle, endpoint, bytes);
        if let Err((error, partial)) = &result {
            if *error == rusb::Error::Pipe {
                let _ = handle.clear_halt(endpoint);
            }
            if *partial {
                // Part of a raster block is in the printer. Left there, it
                // would swallow the next job's bytes and print garbage.
                soft_reset(&handle, interface_number);
            }
        }
        result.map_err(|(error, partial)| describe(error, partial))
    }

    /// `false` when the printer does not answer. Not every printer supports
    /// the request, and a missing answer must not stop the job.
    fn paper_out(handle: &rusb::DeviceHandle<rusb::Context>, interface: u8) -> bool {
        let request_type = rusb::request_type(
            rusb::Direction::In,
            rusb::RequestType::Class,
            rusb::Recipient::Interface,
        );
        let mut status = [0u8; 1];
        matches!(
            handle.read_control(
                request_type,
                GET_PORT_STATUS,
                0,
                u16::from(interface),
                &mut status,
                Duration::from_secs(1),
            ),
            Ok(1) if escpos::port_paper_out(status[0])
        )
    }

    fn soft_reset(handle: &rusb::DeviceHandle<rusb::Context>, interface: u8) {
        let request_type = rusb::request_type(
            rusb::Direction::Out,
            rusb::RequestType::Class,
            rusb::Recipient::Interface,
        );
        let _ = handle.write_control(
            request_type,
            SOFT_RESET,
            0,
            u16::from(interface),
            &[],
            Duration::from_secs(2),
        );
    }

    fn describe(error: rusb::Error, partial: bool) -> JobError {
        let cause = match error {
            rusb::Error::Timeout => {
                "the printer stopped taking data. Check paper, the cover, and that it is on."
                    .to_string()
            }
            rusb::Error::NoDevice => "the printer was unplugged".to_string(),
            other => other.to_string(),
        };
        super::failure(&cause, partial)
    }

    #[cfg(target_os = "linux")]
    struct Reattach<'a> {
        handle: &'a rusb::DeviceHandle<rusb::Context>,
        interface: u8,
    }

    #[cfg(target_os = "linux")]
    impl Drop for Reattach<'_> {
        fn drop(&mut self) {
            // libusb refuses to reattach while the interface is still claimed.
            let _ = self.handle.release_interface(self.interface);
            let _ = self.handle.attach_kernel_driver(self.interface);
        }
    }

    #[cfg(target_os = "linux")]
    fn detach_for_print<'a>(
        handle: &'a rusb::DeviceHandle<rusb::Context>,
        interface: u8,
    ) -> Result<Option<Reattach<'a>>, String> {
        if handle.kernel_driver_active(interface).unwrap_or(false) {
            handle
                .detach_kernel_driver(interface)
                .map_err(|error| error.to_string())?;
            Ok(Some(Reattach { handle, interface }))
        } else {
            Ok(None)
        }
    }

    /// Interface number, alternate setting, and bulk-out endpoint address.
    fn printer_endpoint(config: &rusb::ConfigDescriptor) -> Option<(u8, u8, u8)> {
        for interface in config.interfaces() {
            for alt in interface.descriptors() {
                if alt.class_code() != PRINTER_CLASS {
                    continue;
                }
                if let Some(endpoint) = alt.endpoint_descriptors().find(|endpoint| {
                    endpoint.direction() == rusb::Direction::Out
                        && endpoint.transfer_type() == rusb::TransferType::Bulk
                }) {
                    return Some((
                        alt.interface_number(),
                        alt.setting_number(),
                        endpoint.address(),
                    ));
                }
            }
        }
        None
    }

    /// On failure, also says whether any bytes reached the printer. rusb does
    /// not report how much of a timed-out chunk went out, so a failure on a
    /// later chunk, or a timeout, counts as partial.
    fn write_all(
        handle: &rusb::DeviceHandle<rusb::Context>,
        endpoint: u8,
        bytes: &[u8],
    ) -> Result<(), (rusb::Error, bool)> {
        let timeout = Duration::from_secs(20);
        let mut offset = 0;
        while offset < bytes.len() {
            let end = (offset + 16 * 1024).min(bytes.len());
            let chunk = &bytes[offset..end];
            match handle.write_bulk(endpoint, chunk, timeout) {
                Ok(0) => return Err((rusb::Error::Io, offset > 0)),
                Ok(wrote) => offset += wrote,
                Err(error) => {
                    let partial = offset > 0 || error == rusb::Error::Timeout;
                    return Err((error, partial));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(id: &str, label: &str, place: &str) -> UsbDevice {
        UsbDevice {
            id: id.to_string(),
            label: label.to_string(),
            place: place.to_string(),
        }
    }

    #[test]
    fn the_printer_model_is_read_from_both_kinds_of_id() {
        // Replug follows a lone printer only when its model matches, so the
        // model must come out of a libusb id and a Windows device path alike.
        assert_eq!(model_of("04b8:0202:1-4").as_deref(), Some("04b8:0202"));
        assert_eq!(model_of("04b8:0202:1@7").as_deref(), Some("04b8:0202"));
        assert_eq!(
            model_of(
                r"\\?\usb#vid_04B8&pid_0E15#5&2a0b&0&3#{28d78fad-5a12-11d1-ae5b-0000f803a8c2}"
            )
            .as_deref(),
            Some("04b8:0e15")
        );
        assert_eq!(model_of("printer"), None);
        assert_eq!(model_of("zzzz:0202:1-4"), None);
    }

    #[test]
    fn a_partial_write_says_the_receipt_may_have_printed() {
        let partial = failure("the printer was unplugged", true);
        assert_eq!(partial.fault, Fault::Partial);
        assert!(
            partial
                .message
                .ends_with("unplugged. Part of the receipt may have printed.")
        );
        assert_eq!(failure("refused", false).fault, Fault::Failed);
    }

    #[test]
    fn two_printers_of_the_same_model_get_different_labels() {
        // Two TM-T88V on one till must not look identical in the window.
        let mut found = vec![
            device("b", "USB 04b8:0202", "port 1-2"),
            device("a", "USB 04b8:0202", "port 1-1"),
            device("c", "USB 0519:0001", "port 1-3"),
        ];
        tell_apart(&mut found);
        let labels: Vec<&str> = found.iter().map(|device| device.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "USB 04b8:0202 · port 1-1",
                "USB 04b8:0202 · port 1-2",
                "USB 0519:0001",
            ]
        );
    }
}
