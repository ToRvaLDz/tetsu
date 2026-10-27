# tetsu-comp

The Tetsu Wayland compositor, on Smithay. It maps xdg-shell windows, tiles
them in equal columns (the layout engine replaces this later) and serves
the window and workspace interfaces of the bus
(`org.tetsu.comp.window.v1`, `org.tetsu.comp.workspace.v1`).

## Running

    tetsu-comp [--backend winit|tty|headless] [-- <command>...]

`-- <command>` starts a client with `WAYLAND_DISPLAY` and
`TETSU_COMP_SOCKET` set. Once it listens, `tetsu-comp` prints
`ready WAYLAND_DISPLAY=<name> TETSU_COMP_SOCKET=<path>` on stdout.

- **Nested** (development, inside another Wayland session): the default
  when `WAYLAND_DISPLAY` or `DISPLAY` is set.

      cargo run -p tetsu-comp -- --backend winit -- foot

- **Headless** (CI): a virtual 1920x1080 output, nothing drawn, no input
  device. The tests run the real binary this way, with a real Wayland
  client.

      cargo test -p tetsu-comp

- **On a VT** (`--backend tty`): DRM/KMS on a single GPU, libinput through
  libseat. Run it from a free VT, not inside another session.

      tetsu-comp --backend tty -- foot

  Ctrl+Alt+F1..F12 switch VT, Ctrl+Alt+Backspace quits. Hotplug, multiple
  GPUs and explicit sync are not handled yet.

Query the bus with `varlinkctl call $TETSU_COMP_SOCKET
org.tetsu.comp.window.v1.ListWindows '{}'`. The bus socket is
`$XDG_RUNTIME_DIR/tetsu/comp/<wayland socket name>`, one per instance.

## Build dependencies

Arch: `libinput seatd libxkbcommon xkeyboard-config mesa libdrm wayland`.
Debian and Ubuntu: `pkg-config libudev-dev libseat-dev libinput-dev
libgbm-dev libdrm-dev libxkbcommon-dev libegl-dev`.

## Layout

- `model`: windows, workspaces and the events they emit, in the order the
  contract demands. Pure, tested first.
- `layout`: the default tiling.
- `bus`: the bus thread. The compositor loop sends deltas over an
  unbounded channel and never waits on it; a watcher that falls 256 events
  behind is disconnected, and a dead thread is restarted.
- `state`, `handlers`, `input`, `render`, `backend`: the Smithay side.
