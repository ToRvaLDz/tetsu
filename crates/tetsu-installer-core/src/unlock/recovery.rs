//! Recovery key in the systemd modhex format, and its QR rendering.

use qrcode::QrCode;
use qrcode::render::unicode::Dense1x2;

use crate::entropy::Entropy;
use crate::error::InstallError;
use crate::secret::Secret;

/// systemd's modhex alphabet.
const MODHEX: &[u8; 16] = b"cbdefghijklnrtuv";
const KEY_BYTES: usize = 32;
const GROUP: usize = 8;

/// 32 random bytes as 64 modhex characters in 8 dash-separated groups.
pub fn generate(entropy: &dyn Entropy) -> Result<Secret, InstallError> {
    let mut bytes = [0u8; KEY_BYTES];
    entropy.fill(&mut bytes)?;
    let chars: Vec<char> = bytes
        .iter()
        .flat_map(|b| [b >> 4, b & 0x0f])
        .map(|n| char::from(MODHEX[usize::from(n)]))
        .collect();
    let groups: Vec<String> = chars.chunks(GROUP).map(|c| c.iter().collect()).collect();
    Ok(Secret::new(groups.join("-")))
}

/// Lowercase, without dashes or spaces: what the user typed vs. the key.
pub fn normalise(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .flat_map(char::to_lowercase)
        .collect()
}

/// The QR code as Unicode half blocks, quiet zone included.
pub fn render_qr(payload: &str) -> Result<String, InstallError> {
    let code = QrCode::new(payload.as_bytes()).map_err(|e| InstallError::Qr(e.to_string()))?;
    Ok(code
        .render::<Dense1x2>()
        .dark_color(Dense1x2::Dark)
        .light_color(Dense1x2::Light)
        .build())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entropy::fake::Counter;

    #[test]
    fn key_has_the_systemd_shape() {
        let key = generate(&Counter::default()).unwrap();
        let groups: Vec<&str> = key.expose().split('-').collect();
        assert_eq!(groups.len(), 8);
        assert!(groups.iter().all(|g| g.len() == 8));
        assert!(
            key.expose()
                .chars()
                .all(|c| c == '-' || "cbdefghijklnrtuv".contains(c))
        );
        // Bytes 0, 7, 14, 21 -> cc ci cu bg.
        assert_eq!(groups[0], "cccicubg");
    }

    #[test]
    fn keys_differ_between_calls() {
        let c = Counter::default();
        assert_ne!(generate(&c).unwrap(), generate(&c).unwrap());
    }

    #[test]
    fn normalise_ignores_case_dashes_and_spaces() {
        assert_eq!(normalise(" CC-ci cu "), "cccicu");
    }

    #[test]
    fn qr_is_multi_line_blocks() {
        let qr = render_qr("cccicubg-cccicubg").unwrap();
        assert!(qr.lines().count() > 8);
        assert!(
            qr.chars()
                .all(|c| matches!(c, ' ' | '\n' | '█' | '▀' | '▄'))
        );
        assert!(qr.contains('█') || qr.contains('▀') || qr.contains('▄'));
    }
}
