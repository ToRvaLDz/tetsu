//! Fields of `/proc/self/mountinfo`, shared by the readers of it.

/// Decode the `\040` style octal escapes of mountinfo fields.
pub fn unescape(field: &str) -> String {
    let bytes = field.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let octal = (bytes[i] == b'\\')
            .then(|| bytes.get(i + 1..i + 4))
            .flatten()
            .filter(|d| d.iter().all(|c| (b'0'..=b'7').contains(c)))
            .and_then(|d| u8::from_str_radix(std::str::from_utf8(d).ok()?, 8).ok());
        match octal {
            Some(b) => {
                out.push(b);
                i += 4;
            }
            None => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn octal_escapes_are_decoded_and_the_rest_kept() {
        assert_eq!(unescape("/sys\\040root"), "/sys root");
        assert_eq!(unescape("a\\0b\\"), "a\\0b\\");
    }
}
