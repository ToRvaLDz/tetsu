//! A minimal EDID parser: make, model and serial for `output.v1`.
//!
//! The blob comes from the display, so every field is untrusted: control
//! and non-ASCII characters are dropped and each field is cut short.

/// The longest field kept, in characters.
pub const MAX_FIELD: usize = 32;
/// One EDID block.
const BLOCK: usize = 128;
const HEADER: [u8; 8] = [0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0];
/// The four 18-byte descriptors start here.
const DESCRIPTORS: usize = 54;
const DESCRIPTOR_LEN: usize = 18;
const TAG_NAME: u8 = 0xfc;
const TAG_SERIAL: u8 = 0xff;

/// What the EDID says about the display; `None` when it says nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EdidInfo {
    /// The three-letter PNP manufacturer id, for example `GSM`.
    pub make: Option<String>,
    /// The product name.
    pub model: Option<String>,
    /// The serial string, else the serial number as hex.
    pub serial: Option<String>,
}

/// Parses the first block. A short blob, a bad header or a bad checksum
/// gives nothing.
pub fn parse(blob: &[u8]) -> EdidInfo {
    let Some(block) = blob.get(..BLOCK) else {
        return EdidInfo::default();
    };
    let sum = block.iter().fold(0u8, |a, b| a.wrapping_add(*b));
    if block[..8] != HEADER || sum != 0 {
        return EdidInfo::default();
    }
    let number = u32::from_le_bytes([block[12], block[13], block[14], block[15]]);
    let serial = descriptor_text(block, TAG_SERIAL)
        .or_else(|| (number != 0).then(|| format!("0x{number:08x}")));
    EdidInfo {
        make: pnp_id(block[8], block[9]),
        model: descriptor_text(block, TAG_NAME),
        serial,
    }
}

/// Three letters of five bits each, `A` being 1.
fn pnp_id(high: u8, low: u8) -> Option<String> {
    let id = u16::from_be_bytes([high, low]);
    let letters: Option<String> = [10, 5, 0]
        .into_iter()
        .map(|shift| match (id >> shift) & 0x1f {
            n @ 1..=26 => Some(char::from(b'A' + n as u8 - 1)),
            _ => None,
        })
        .collect();
    letters
}

/// The text of the first descriptor with `tag`.
fn descriptor_text(block: &[u8], tag: u8) -> Option<String> {
    (0..4).find_map(|slot| {
        let at = DESCRIPTORS + slot * DESCRIPTOR_LEN;
        let d = &block[at..at + DESCRIPTOR_LEN];
        if d[..3] != [0, 0, 0] || d[3] != tag {
            return None;
        }
        let text = &d[5..];
        let end = text.iter().position(|b| *b == b'\n').unwrap_or(text.len());
        clean(&String::from_utf8_lossy(&text[..end]))
    })
}

/// Keeps printable ASCII, trims and cuts to `MAX_FIELD`; `None` when
/// nothing is left.
pub fn clean(text: &str) -> Option<String> {
    let kept: String = text
        .chars()
        .filter(|c| c.is_ascii() && !c.is_ascii_control())
        .collect();
    let kept: String = kept.trim().chars().take(MAX_FIELD).collect();
    (!kept.is_empty()).then_some(kept)
}
