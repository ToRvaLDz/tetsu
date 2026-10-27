# tetsu-comp

The Tetsu Wayland compositor, on Smithay. It maps xdg-shell windows, lays
them out (dwindle, master or floating, with window rules) and serves the
window and workspace interfaces of the bus (`org.tetsu.comp.window.v1`,
`org.tetsu.comp.workspace.v1`).

## Running

    tetsu-comp [--backend winit|tty|headless] [--config <file>]
               [--headless-outputs <1..4>] [-- <command>...]

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

- **`--headless-outputs <n>`** (headless only, 1 to 4): adds
  `HEADLESS-2`.. to the right of the first output. It exists so that tests
  can exercise the `output` window rule.

Query the bus with `varlinkctl call $TETSU_COMP_SOCKET
org.tetsu.comp.window.v1.ListWindows '{}'`. The bus socket is
`$XDG_RUNTIME_DIR/tetsu/comp/<wayland socket name>`, one per instance.

## Build dependencies

Arch: `libinput seatd libxkbcommon xkeyboard-config mesa libdrm wayland`.
Debian and Ubuntu: `pkg-config libudev-dev libseat-dev libinput-dev
libgbm-dev libdrm-dev libxkbcommon-dev libegl-dev`.

## Layouts

Each workspace has its own layout. The default comes from the config
(`layout.default`); Mod+Space cycles dwindle, master, floating.

- **dwindle**: each window takes half of what is left, split along the
  longer side; the last one takes the rest. New windows are appended.
- **master**: the first window is the master on the left (ratio 0.55,
  step 0.05, clamped to 0.1..0.9), the rest stack on the right in equal
  rows.
- **floating**: every window floats. A new one is centred at its own size.

Any window can also float on its own (Mod+F). A tile that floats keeps its
size, capped to 3/4 of the output, and reuses its last floating place.
Floating windows stay above the tiles. Workspaces 1 to 9 exist per output
once used; they are never removed yet.

## Binds

The modifier (Mod) is **Super** on a VT and headless, and **Ctrl+Alt**
nested in winit, because the host compositor grabs Super. The table is not
configurable yet.

| Keys | Action |
|---|---|
| Mod+J / Mod+K | focus next / previous window |
| Mod+Shift+J / Mod+Shift+K | swap with next / previous |
| Mod+Shift+Return | make the focused window the master |
| Mod+H / Mod+L | master ratio -/+ (master layout) |
| Mod+Space | cycle the layout |
| Mod+F | toggle floating |
| Mod+1..9 | show workspace N |
| Mod+Shift+1..9 | send the focused window to workspace N (stay here) |
| Mod+Q | close the focused window |
| Mod+left-drag | move a floating window |
| Ctrl+Alt+Backspace | quit |
| Ctrl+Alt+F1..F12 | switch VT (tty) |

## Config

`--config <file>` is an optional TOML file. Without it: dwindle, no gaps,
master ratio 0.55, no rules. Unknown keys, bad regexes and out-of-range
values stop the start with a message naming the key (exit code 2).

    [layout]
    default      = "dwindle"   # dwindle | master | floating
    gaps_outer   = 8
    gaps_inner   = 8
    master_ratio = 0.55        # 0.1 to 0.9

    # All rules that match are merged in file order; the later one wins per
    # field. Matchers are unanchored regexes: use ^...$ to anchor.
    [[window_rule]]
    app_id   = "^pavucontrol$"   # and/or title = "..."
    floating = true
    size     = [640, 480]        # configure size, floating windows
    # workspace = 3              # open on workspace 3 (1..9), unfocused
    # output    = "HEADLESS-2"   # open on that output's active workspace

A window that opens on another workspace or output than the focused one
does not take the focus. Rules are evaluated at the first configure and at
map, not on later title changes.

## Modules

- `model`: windows, workspaces and the events they emit, in the order the
  contract demands. Pure, tested first.
- `layout/`: the algorithms (`dwindle`, `master`, `floating`). Pure.
- `rules`: app_id and title patterns to a decision. Pure.
- `settings`: the config file.
- `engine`: the model plus per-workspace layout state; `apply(Action)`
  and `arrange` give events and rects. Pure.
- `grab`: the pointer grab that moves a floating window.
- `bus`: the bus thread. The compositor loop sends deltas over an
  unbounded channel and never waits on it; a watcher that falls 256 events
  behind is disconnected, and a dead thread is restarted.
- `state`, `handlers`, `input`, `render`, `backend`: the Smithay side.
