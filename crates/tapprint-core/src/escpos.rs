//! Receipt bytes for a thermal printer that understands the public raster command.
//!
//! `GS v 0` is the common "print raster bit image" command. The bytes are
//! produced here; nothing is copied from a vendor driver.

use crate::raster::MonoRaster;

const INIT: [u8; 2] = [0x1b, 0x40];
/// `GS V 66 48`: feed 48 vertical motion units, then a partial cut. The
/// unit is set by `GS P`; on the TM-T88V the default is 1/360 inch, so about 3.4 mm.
const CUT: [u8; 4] = [0x1d, 0x56, 0x42, 48];
const ROWS_PER_COMMAND: u32 = 128;
/// `DLE EOT 4`: ask for the roll paper sensor. Real-time, so the printer
/// answers even while it is offline for lack of paper.
pub const PAPER_STATUS: [u8; 3] = [0x10, 0x04, 0x04];
pub const PAPER_OUT: &str = "the printer is out of paper. Load a new roll, close the cover, then print again. Nothing was printed.";

#[derive(Debug)]
pub struct EscposError(pub String);

/// `ESC p m t1 t2`: pulse drawer pin 2 (`m = 0`) or pin 5 (`m = 1`). The
/// times are in 2 ms steps. Drawers differ, so both come from the settings.
pub fn drawer_command(pin: u8, on_ms: u16, off_ms: u16) -> [u8; 5] {
    let m = u8::from(pin == 5);
    let steps = |ms: u16| u8::try_from((ms / 2).clamp(1, 255)).unwrap_or(u8::MAX);
    [0x1b, 0x70, m, steps(on_ms), steps(off_ms)]
}

/// Kick the drawer without printing a slip.
pub fn drawer_pulse(drawer: [u8; 5]) -> Vec<u8> {
    let mut out = Vec::with_capacity(INIT.len() + drawer.len());
    out.extend_from_slice(&INIT);
    out.extend_from_slice(&drawer);
    out
}

/// Reply to `DLE EOT 4`. `None` when the byte is not a status reply, so a
/// printer that sends something else is never reported as out of paper.
pub fn roll_paper_out(reply: u8) -> Option<bool> {
    // Bits 1 and 4 are always 1, bits 0 and 7 always 0. Bits 5 and 6 mean paper end.
    (reply & 0x93 == 0x12).then_some(reply & 0x60 == 0x60)
}

/// USB printer-class port status: bit 5 is PaperEmpty, bit 3 is nError.
/// Only trust paper empty when the printer also reports an error, because
/// some printers leave bit 5 set while they print fine.
pub fn port_paper_out(status: u8) -> bool {
    status & 0x20 != 0 && status & 0x08 == 0
}

/// Build one job: reset, raster in short bands, then an optional drawer kick and cut.
pub fn encode(
    raster: &MonoRaster,
    cut: bool,
    drawer: Option<[u8; 5]>,
) -> Result<Vec<u8>, EscposError> {
    let width_bytes = usize::from(raster.width_bytes);
    if width_bytes == 0 || raster.height == 0 {
        return Err(EscposError("nothing to print".to_string()));
    }
    let expected = width_bytes.saturating_mul(raster.height as usize);
    if raster.packed.len() != expected {
        return Err(EscposError(
            "raster size does not match its width".to_string(),
        ));
    }

    let mut out = Vec::with_capacity(expected + 64);
    out.extend_from_slice(&INIT);
    let mut row = 0u32;
    while row < raster.height {
        let band = ROWS_PER_COMMAND.min(raster.height - row);
        let y = band.to_le_bytes();
        let x = raster.width_bytes.to_le_bytes();
        out.extend_from_slice(&[0x1d, 0x76, 0x30, 0x00, x[0], x[1], y[0], y[1]]);
        let start = width_bytes * row as usize;
        let end = start + width_bytes * band as usize;
        let Some(slice) = raster.packed.get(start..end) else {
            return Err(EscposError("raster band ran past the image".to_string()));
        };
        out.extend_from_slice(slice);
        row += band;
    }
    if let Some(drawer) = drawer {
        out.extend_from_slice(&drawer);
    }
    if cut {
        out.extend_from_slice(&CUT);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_single_row_is_one_raster_command_then_a_cut() {
        let raster = MonoRaster {
            width_bytes: 1,
            height: 1,
            packed: vec![0xff],
        };
        let bytes = encode(&raster, true, None).unwrap();
        assert_eq!(&bytes[..2], &[0x1b, 0x40]);
        assert_eq!(
            &bytes[2..10],
            &[0x1d, 0x76, 0x30, 0x00, 0x01, 0x00, 0x01, 0x00]
        );
        assert_eq!(bytes[10], 0xff);
        assert_eq!(&bytes[11..], &[0x1d, 0x56, 0x42, 48]);
    }

    #[test]
    fn tall_images_are_split_into_bands() {
        let raster = MonoRaster {
            width_bytes: 1,
            height: 200,
            packed: vec![0x00; 200],
        };
        let bytes = encode(&raster, false, None).unwrap();
        let commands = bytes
            .windows(4)
            .filter(|w| *w == [0x1d, 0x76, 0x30, 0x00])
            .count();
        assert_eq!(commands, 2);
    }

    #[test]
    fn the_drawer_pulse_is_the_public_pin_command() {
        // The default, pin 2 for 50 ms, is what most drawers expect.
        let pulse = drawer_pulse(drawer_command(2, 50, 50));
        assert_eq!(pulse, vec![0x1b, 0x40, 0x1b, 0x70, 0x00, 25, 25]);
        let raster = MonoRaster {
            width_bytes: 1,
            height: 1,
            packed: vec![0x00],
        };
        let bytes = encode(&raster, true, Some(drawer_command(2, 50, 50))).unwrap();
        assert!(bytes.ends_with(&[0x1b, 0x70, 0x00, 25, 25, 0x1d, 0x56, 0x42, 48]));
    }

    #[test]
    fn a_drawer_on_pin_5_with_a_longer_pulse_gets_its_own_command() {
        // Some drawers are wired to pin 5, and heavy ones need a longer kick.
        assert_eq!(drawer_command(5, 120, 240), [0x1b, 0x70, 1, 60, 120]);
        // Out of range times are held to what the command can carry.
        assert_eq!(drawer_command(2, 0, 9_000), [0x1b, 0x70, 0, 1, 255]);
    }

    #[test]
    fn only_a_real_status_reply_can_say_the_paper_is_out() {
        assert_eq!(roll_paper_out(0x12), Some(false));
        assert_eq!(roll_paper_out(0x1e), Some(false)); // near end still prints
        assert_eq!(roll_paper_out(0x72), Some(true));
        // A stray byte, such as an ASCII letter, is not a status reply.
        assert_eq!(roll_paper_out(b'A'), None);
        assert_eq!(roll_paper_out(0xff), None);

        assert!(port_paper_out(0x20 | 0x10));
        assert!(!port_paper_out(0x18)); // selected, no error
        assert!(!port_paper_out(0x20 | 0x18)); // paper bit with no error is not trusted
    }
}
