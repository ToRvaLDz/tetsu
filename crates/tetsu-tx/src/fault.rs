//! Pause points: a test hook that stops a transaction at a named boundary
//! until the harness removes the pause file. The VM suite cuts the power
//! while the transaction waits, so every step of the commit protocol gets
//! its own power-loss test. The hook only waits; it never changes what a
//! transaction writes, and it does nothing unless `TETSU_TX_PAUSE_AT` names
//! a point.

use std::fs;
use std::path::Path;
use std::sync::OnceLock;
use std::thread;
use std::time::Duration;

/// How often a paused transaction looks for the pause file to be gone.
const POLL: Duration = Duration::from_millis(200);

/// Environment variable naming the point to pause at.
pub const PAUSE_ENV: &str = "TETSU_TX_PAUSE_AT";
/// Written with the point name while a transaction waits; removing it lets
/// the transaction go on.
pub const PAUSE_FILE: &str = "/run/tetsu/paused-at";

/// A boundary of the commit protocol, in protocol order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Point {
    /// After the collection of leftovers, before the snapshot.
    GcDone,
    /// The new snapshot exists.
    Snapshot,
    /// The packages are downloaded.
    Downloaded,
    /// The packages are installed, the snapshot is not sealed yet.
    Installed,
    /// The snapshot is sealed read-only.
    Sealed,
    /// `tetsu-<n>.efi.tmp` is written and synced.
    UkiStaged,
    /// Renamed into place, the directory is not synced yet.
    Renamed,
    /// The entry is committed and synced.
    Committed,
    /// The record is written, retention has not run.
    Recorded,
}

impl Point {
    /// Every point, in protocol order.
    pub const ALL: [Point; 9] = [
        Point::GcDone,
        Point::Snapshot,
        Point::Downloaded,
        Point::Installed,
        Point::Sealed,
        Point::UkiStaged,
        Point::Renamed,
        Point::Committed,
        Point::Recorded,
    ];

    /// The name used in the environment and in the log line.
    pub fn name(self) -> &'static str {
        match self {
            Self::GcDone => "gc-done",
            Self::Snapshot => "snapshot",
            Self::Downloaded => "downloaded",
            Self::Installed => "installed",
            Self::Sealed => "sealed",
            Self::UkiStaged => "uki-staged",
            Self::Renamed => "renamed",
            Self::Committed => "committed",
            Self::Recorded => "recorded",
        }
    }

    /// The point called `name`.
    pub fn parse(name: &str) -> Result<Point, String> {
        Self::ALL
            .into_iter()
            .find(|p| p.name() == name)
            .ok_or_else(|| {
                let valid: Vec<&str> = Self::ALL.iter().map(|p| p.name()).collect();
                format!(
                    "unknown pause point {name} in {PAUSE_ENV} (valid: {})",
                    valid.join(", ")
                )
            })
    }
}

/// The point named by `value` of [`PAUSE_ENV`]; `None` when unset or empty.
pub fn armed_from(value: Option<&str>) -> Result<Option<Point>, String> {
    match value {
        None | Some("") => Ok(None),
        Some(name) => Point::parse(name).map(Some),
    }
}

/// Wait at `point` when it is the `armed` one: write `file` with the name,
/// say so on stderr, and poll until the file is gone.
pub fn pause_at(point: Point, armed: Option<Point>, file: &Path) {
    if armed != Some(point) {
        return;
    }
    if let Some(dir) = file.parent() {
        let _ = fs::create_dir_all(dir);
    }
    if let Err(e) = fs::write(file, format!("{}\n", point.name())) {
        // Without the file nobody can release the wait: go on.
        eprintln!("tetsu-tx: cannot write {}: {e}", file.display());
        return;
    }
    eprintln!("tetsu-tx: paused at {}", point.name());
    while file.exists() {
        thread::sleep(POLL);
    }
}

/// The point the CLI armed, if any.
static ARMED: OnceLock<Point> = OnceLock::new();

/// Arm `point` for this process; only the CLI calls it.
pub fn arm(point: Point) {
    let _ = ARMED.set(point);
}

#[cfg(test)]
thread_local! {
    /// The points reached on this thread, for engine tests.
    static SEEN: std::cell::RefCell<Vec<Point>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// The points reached on this thread so far; clears the list.
#[cfg(test)]
pub(crate) fn take_seen() -> Vec<Point> {
    SEEN.with(|s| std::mem::take(&mut *s.borrow_mut()))
}

/// The transaction is at `point`: wait here when it is the armed one.
pub fn reach(point: Point) {
    #[cfg(test)]
    SEEN.with(|s| s.borrow_mut().push(point));
    pause_at(point, ARMED.get().copied(), Path::new(PAUSE_FILE));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::Duration;

    #[test]
    fn the_points_are_the_nine_names_in_protocol_order() {
        let names: Vec<&str> = Point::ALL.iter().map(|p| p.name()).collect();
        assert_eq!(
            names,
            [
                "gc-done",
                "snapshot",
                "downloaded",
                "installed",
                "sealed",
                "uki-staged",
                "renamed",
                "committed",
                "recorded"
            ]
        );
        for p in Point::ALL {
            assert_eq!(Point::parse(p.name()), Ok(p));
        }
    }

    #[test]
    fn an_unknown_name_is_rejected_and_names_the_valid_ones() {
        let err = Point::parse("later").unwrap_err();
        assert!(err.contains("later"), "{err}");
        assert!(err.contains("uki-staged"), "{err}");
        assert!(armed_from(Some("later")).is_err());
        assert_eq!(armed_from(None), Ok(None));
        assert_eq!(armed_from(Some("")), Ok(None));
        assert_eq!(armed_from(Some("sealed")), Ok(Some(Point::Sealed)));
    }

    #[test]
    fn another_point_returns_at_once_and_leaves_no_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("paused-at");
        pause_at(Point::Sealed, Some(Point::Renamed), &file);
        pause_at(Point::Sealed, None, &file);
        assert!(!file.exists());
    }

    #[test]
    fn the_armed_point_blocks_until_the_file_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("run").join("paused-at");
        let waiter = {
            let file = file.clone();
            std::thread::spawn(move || pause_at(Point::Renamed, Some(Point::Renamed), &file))
        };
        for _ in 0..100 {
            if file.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(fs::read_to_string(&file).unwrap().trim(), "renamed");
        std::thread::sleep(Duration::from_millis(400));
        assert!(!waiter.is_finished());
        fs::remove_file(&file).unwrap();
        waiter.join().unwrap();
    }
}
