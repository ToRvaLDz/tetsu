//! The installation log: kept in memory, shown by the frontend and, once
//! the ESP is mounted, written to a file on it. Callers pass descriptions
//! only, never secrets.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;

use crate::error::InstallError;

/// File name of the log on the ESP.
pub const LOG_FILE: &str = "tetsu-install.log";

/// Lines buffered until a file is attached, then mirrored into it.
#[derive(Debug, Default)]
pub struct InstallLog {
    lines: Vec<String>,
    file: Option<File>,
}

impl InstallLog {
    pub fn new() -> Self {
        Self::default()
    }

    /// Every line recorded so far.
    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    /// Record a line. A failing log file is dropped: the install is never
    /// failed over its log, and the memory copy stays complete.
    pub fn record(&mut self, line: &str) {
        self.lines.push(line.to_owned());
        if let Some(file) = &mut self.file
            && writeln!(file, "{line}")
                .and_then(|()| file.flush())
                .is_err()
        {
            self.file = None;
        }
    }

    /// Stop mirroring and close the file (before the ESP is unmounted).
    pub fn detach(&mut self) {
        if let Some(file) = self.file.take() {
            let _ = file.sync_all();
        }
    }

    /// Start mirroring into `path` and flush everything buffered so far.
    pub fn attach(&mut self, path: &Path) -> Result<(), InstallError> {
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(path)
            .map_err(|e| InstallError::io(path, e))?;
        for line in &self.lines {
            writeln!(file, "{line}").map_err(|e| InstallError::io(path, e))?;
        }
        file.sync_all().map_err(|e| InstallError::io(path, e))?;
        self.file = Some(file);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn buffers_until_attached_then_flushes_and_follows() {
        let t = tempfile::tempdir().unwrap();
        let path = t.path().join(LOG_FILE);
        let mut log = InstallLog::new();
        log.record("one");
        log.record("two");
        assert!(!path.exists());
        log.attach(&path).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "one\ntwo\n");
        log.record("three");
        assert_eq!(fs::read_to_string(&path).unwrap(), "one\ntwo\nthree\n");
        assert_eq!(log.lines(), ["one", "two", "three"]);
    }

    #[test]
    fn detach_stops_the_mirror_but_keeps_the_memory_copy() {
        let t = tempfile::tempdir().unwrap();
        let path = t.path().join(LOG_FILE);
        let mut log = InstallLog::new();
        log.attach(&path).unwrap();
        log.record("one");
        log.detach();
        log.record("two");
        assert_eq!(fs::read_to_string(&path).unwrap(), "one\n");
        assert_eq!(log.lines(), ["one", "two"]);
    }

    #[test]
    fn attach_failure_is_reported_and_the_buffer_kept() {
        let t = tempfile::tempdir().unwrap();
        let mut log = InstallLog::new();
        log.record("one");
        assert!(log.attach(&t.path().join("no/such/dir/log")).is_err());
        log.record("two");
        assert_eq!(log.lines(), ["one", "two"]);
    }
}
