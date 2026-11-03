//! What a bind asks the engine to do.

/// An operation on the focused workspace or window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Focus the next window in layout order, wrapping.
    FocusNext,
    /// Focus the previous window in layout order, wrapping.
    FocusPrev,
    /// Swap the focused window with the next one.
    SwapNext,
    /// Swap the focused window with the previous one.
    SwapPrev,
    /// Make the focused window the master.
    PromoteMaster,
    /// Change the master ratio by this many steps (master layout only).
    RatioDelta(i8),
    /// Dwindle, master, floating, dwindle...
    CycleLayout,
    /// Float or tile the focused window.
    ToggleFloating,
    /// Show workspace 1..=9 of the focused output.
    FocusWorkspace(u8),
    /// Send the focused window to workspace 1..=9, without following it.
    MoveToWorkspace(u8),
    /// Ask the focused window to close.
    Close,
}

/// What the glue has to do after an action.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Effect {
    /// The arrangement changed: lay the windows out again.
    pub relayout: bool,
    /// A window to send a close request to.
    pub close: Option<String>,
}
