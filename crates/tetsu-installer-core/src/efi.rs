//! Reading the Platform Key from the firmware, to tell whether it is ours.
//! Used by `tetsu secureboot trust` and by the first-boot seal in the
//! initrd. All reads happen under a root directory, so tests use a tempdir.

use std::path::Path;

use crate::error::InstallError;
use crate::preflight::GLOBAL_VARIABLE_GUID;

/// `EFI_CERT_X509_GUID` (a5c059a1-94e4-4aa7-87b5-ab155c2bf072) as stored.
const X509_GUID: [u8; 16] = [
    0xa1, 0x59, 0xc0, 0xa5, 0xe4, 0x94, 0xa7, 0x4a, 0x87, 0xb5, 0xab, 0x15, 0x5c, 0x2b, 0xf0, 0x72,
];
/// Signature list header: type GUID, list size, header size, signature size.
const LIST_HEADER: usize = 28;
const OWNER_GUID_LEN: usize = 16;

fn u32_at(b: &[u8], at: usize) -> Option<usize> {
    let raw: [u8; 4] = b.get(at..at + 4)?.try_into().ok()?;
    usize::try_from(u32::from_le_bytes(raw)).ok()
}

/// The X.509 certificates (DER) of an EFI signature database, as `efivarfs`
/// shows it without the four attribute bytes. Entries that do not fit are
/// ignored, whatever the firmware put there.
pub fn x509_certificates(mut data: &[u8]) -> Vec<Vec<u8>> {
    let mut found = Vec::new();
    while data.len() >= LIST_HEADER {
        let (Some(list), Some(header), Some(sig)) =
            (u32_at(data, 16), u32_at(data, 20), u32_at(data, 24))
        else {
            break;
        };
        if list < LIST_HEADER || list > data.len() {
            break;
        }
        if data[..16] == X509_GUID && sig > OWNER_GUID_LEN {
            let body = LIST_HEADER.saturating_add(header);
            if let Some(entries) = data.get(body..list) {
                for entry in entries.chunks_exact(sig) {
                    found.push(entry[OWNER_GUID_LEN..].to_vec());
                }
            }
        }
        data = &data[list..];
    }
    found
}

/// The certificates in the Platform Key variable; none when it is absent.
pub fn platform_key_certificates(root: &Path) -> Vec<Vec<u8>> {
    let path = root
        .join("sys/firmware/efi/efivars")
        .join(format!("PK-{GLOBAL_VARIABLE_GUID}"));
    match std::fs::read(path) {
        Ok(bytes) if bytes.len() > 4 => x509_certificates(&bytes[4..]),
        _ => Vec::new(),
    }
}

fn base64_value(c: u8) -> Option<u8> {
    match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in text.bytes().filter(|c| !c.is_ascii_whitespace()) {
        if c == b'=' {
            break;
        }
        acc = (acc << 6) | u32::from(base64_value(c)?);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(u8::try_from((acc >> bits) & 0xff).ok()?);
        }
    }
    Some(out)
}

/// The DER of the first certificate in `bytes`: a PEM block is decoded,
/// anything else is taken as DER already.
pub fn certificate_der(bytes: &[u8]) -> Option<Vec<u8>> {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return Some(bytes.to_vec());
    };
    let Some(begin) = text.find("-----BEGIN CERTIFICATE-----") else {
        return Some(bytes.to_vec());
    };
    let body = &text[begin + "-----BEGIN CERTIFICATE-----".len()..];
    let end = body.find("-----END CERTIFICATE-----")?;
    base64_decode(&body[..end]).filter(|der| !der.is_empty())
}

/// Whether the Platform Key holds the certificate in `cert` (PEM or DER).
pub fn platform_key_is(root: &Path, cert: &Path) -> Result<bool, InstallError> {
    let bytes = std::fs::read(cert).map_err(|e| InstallError::io(cert, e))?;
    let der = certificate_der(&bytes)
        .ok_or_else(|| InstallError::Failed(format!("{}: not a certificate", cert.display())))?;
    Ok(platform_key_certificates(root).contains(&der))
}

#[cfg(test)]
pub(crate) mod tests_support {
    use super::*;

    /// A signature list with one X.509 entry per certificate.
    pub fn esl(certs: &[&[u8]]) -> Vec<u8> {
        let sig = u32::try_from(OWNER_GUID_LEN + certs[0].len()).unwrap();
        let list = u32::try_from(LIST_HEADER).unwrap() + sig * u32::try_from(certs.len()).unwrap();
        let mut out = X509_GUID.to_vec();
        out.extend(list.to_le_bytes());
        out.extend(0u32.to_le_bytes());
        out.extend(sig.to_le_bytes());
        for cert in certs {
            out.extend([7u8; OWNER_GUID_LEN]);
            out.extend(*cert);
        }
        out
    }

    /// Write the Platform Key variable under `root`.
    pub fn write_pk(root: &Path, esl: &[u8]) {
        let dir = root.join("sys/firmware/efi/efivars");
        std::fs::create_dir_all(&dir).unwrap();
        let mut file = vec![7, 0, 0, 0];
        file.extend(esl);
        std::fs::write(dir.join(format!("PK-{GLOBAL_VARIABLE_GUID}")), file).unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::tests_support::*;
    use super::*;

    // "MIIB" is a base64 quartet: bytes 0x30 0x82 0x01.
    const PEM: &str = "-----BEGIN CERTIFICATE-----\nMIIB\nAQID\n-----END CERTIFICATE-----\n";
    const DER: [u8; 6] = [0x30, 0x82, 0x01, 0x01, 0x02, 0x03];

    #[test]
    fn pem_is_decoded_and_der_is_kept() {
        assert_eq!(certificate_der(PEM.as_bytes()), Some(DER.to_vec()));
        assert_eq!(certificate_der(&DER), Some(DER.to_vec()));
        assert_eq!(certificate_der(b"-----BEGIN CERTIFICATE-----\nMIIB"), None);
        assert_eq!(
            base64_decode("TWFu TWE=").unwrap(),
            b"ManMa".to_vec(),
            "whitespace and padding"
        );
        assert_eq!(base64_decode("TW!u"), None);
    }

    #[test]
    fn certificates_are_read_from_every_entry_and_list() {
        let mut data = esl(&[&[1, 2, 3], &[4, 5, 6]]);
        data.extend(esl(&[&[9, 9, 9]]));
        assert_eq!(
            x509_certificates(&data),
            [vec![1, 2, 3], vec![4, 5, 6], vec![9, 9, 9]]
        );
    }

    #[test]
    fn other_list_types_and_broken_lists_are_ignored() {
        let mut sha = esl(&[&[1, 2, 3]]);
        sha[0] ^= 0xff;
        assert!(x509_certificates(&sha).is_empty());
        let good = esl(&[&[1, 2, 3]]);
        assert!(x509_certificates(&good[..good.len() - 1]).is_empty());
        assert!(x509_certificates(&[]).is_empty());
        let mut zero = good.clone();
        zero[24..28].copy_from_slice(&0u32.to_le_bytes());
        assert!(x509_certificates(&zero).is_empty());
        let mut huge = good;
        huge[20..24].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(x509_certificates(&huge).is_empty());
    }

    #[test]
    fn the_platform_key_is_compared_with_a_pem_or_der_file() {
        let t = tempfile::tempdir().unwrap();
        let pem = t.path().join("PK.crt");
        std::fs::write(&pem, PEM).unwrap();
        assert!(!platform_key_is(t.path(), &pem).unwrap());
        write_pk(t.path(), &esl(&[&DER]));
        assert!(platform_key_is(t.path(), &pem).unwrap());
        let der = t.path().join("PK.der");
        std::fs::write(&der, DER).unwrap();
        assert!(platform_key_is(t.path(), &der).unwrap());
        write_pk(t.path(), &esl(&[&[1, 1, 1]]));
        assert!(!platform_key_is(t.path(), &pem).unwrap());
        assert!(platform_key_is(t.path(), &t.path().join("none")).is_err());
    }
}
