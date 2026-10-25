//! Random bytes behind a trait, so generated values are testable.

use std::io::Read;

use crate::error::InstallError;

/// Source of random bytes.
pub trait Entropy {
    fn fill(&self, buf: &mut [u8]) -> Result<(), InstallError>;
}

/// Reads `/dev/urandom`.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemEntropy;

impl Entropy for SystemEntropy {
    fn fill(&self, buf: &mut [u8]) -> Result<(), InstallError> {
        let path = "/dev/urandom";
        std::fs::File::open(path)
            .and_then(|mut f| f.read_exact(buf))
            .map_err(|e| InstallError::io(path, e))
    }
}

#[cfg(test)]
pub(crate) mod fake {
    use std::cell::Cell;

    use super::*;

    /// Deterministic counter bytes.
    #[derive(Default)]
    pub struct Counter(pub Cell<u8>);

    impl Entropy for Counter {
        fn fill(&self, buf: &mut [u8]) -> Result<(), InstallError> {
            for b in buf {
                *b = self.0.get();
                self.0.set(self.0.get().wrapping_add(7));
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::Counter;
    use super::*;

    #[test]
    fn counter_is_deterministic() {
        let mut buf = [0u8; 3];
        Counter::default().fill(&mut buf).unwrap();
        assert_eq!(buf, [0, 7, 14]);
    }

    #[test]
    fn system_entropy_fills_the_buffer() {
        let mut a = [0u8; 32];
        let mut b = [0u8; 32];
        SystemEntropy.fill(&mut a).unwrap();
        SystemEntropy.fill(&mut b).unwrap();
        assert_ne!(a, b);
    }
}
