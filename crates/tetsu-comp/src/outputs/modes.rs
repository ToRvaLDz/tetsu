//! The mode list of an output, as `output.v1` states it.

use tetsu_bus::output::Mode;

/// A mode as the backends report it: width, height, refresh in
/// millihertz, preferred.
pub type RawMode = (i32, i32, i32, bool);

/// Drops duplicate modes (the first one wins, its `preferred` flag too) and
/// finds the current mode, given as width, height and millihertz, in the
/// result. `None` when it is not listed or the list is empty.
pub fn from_list(raw: &[RawMode], current: (i32, i32, i32)) -> (Vec<Mode>, Option<usize>) {
    let mut modes: Vec<Mode> = Vec::new();
    for &(w, h, refresh, preferred) in raw {
        let mode = Mode {
            width: i64::from(w),
            height: i64::from(h),
            refresh_mhz: i64::from(refresh),
            preferred,
        };
        let same = |m: &Mode| (m.width, m.height, m.refresh_mhz) == key(&mode);
        if !modes.iter().any(same) {
            modes.push(mode);
        }
    }
    let wanted = (
        i64::from(current.0),
        i64::from(current.1),
        i64::from(current.2),
    );
    let at = modes.iter().position(|m| key(m) == wanted);
    (modes, at)
}

fn key(mode: &Mode) -> (i64, i64, i64) {
    (mode.width, mode.height, mode.refresh_mhz)
}
