//! X11 clients through xwayland-satellite: the compositor owns an X11
//! display (`display`) and starts satellite on its first connection;
//! satellite is an ordinary Wayland client, told apart by its pid
//! (`satellite`).

pub mod display;
pub mod satellite;
