# tetsu-comp

The Tetsu Wayland compositor, on Smithay. It maps xdg-shell windows, lays
them out (dwindle, master or floating, with window rules) and serves the
window, workspace and output interfaces of the bus
(`org.tetsu.comp.window.v1`, `org.tetsu.comp.workspace.v1`,
`org.tetsu.comp.output.v1`), and `zwlr_layer_shell_v1` for the bar.

## Running

    tetsu-comp [--backend winit|tty|headless] [--config <file>]
               [--headless-outputs <1..4>] [--headless-hotplug]
               [-- <command>...]

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

- **On a VT** (`--backend tty`): DRM/KMS on every GPU of the seat, libinput
  through libseat. Run it from a free VT, not inside another session.

      tetsu-comp --backend tty -- foot

  Ctrl+Alt+F1..F12 switch VT, Ctrl+Alt+Backspace quits.

  **Multi-GPU.** One render device composes every output; an output on
  another card gets a cross-GPU copy (8-bit formats only, linear buffers
  for a display-only card, no overlay planes). The render device is chosen
  once, at start: `render.device` if it names a present GPU, else the
  hardware GPU that drives a connected external output, then the one with
  most connected outputs, then the udev primary, then the lowest id. A
  software renderer (llvmpipe, as on QEMU's std VGA) is used only when it
  is the only device. The log says what was chosen:

      tetsu-comp: render device renderD128 (i915), automatic: only device
      tetsu-comp: output eDP-1 on card0, cross-gpu=false

  **Hotplug.** Monitors come and go without a restart (see Outputs). The
  cards are fixed at start: a udev `change` on a known card, and a
  session resume, look at its connectors again; a card added later is only
  logged (`card <path> added; restart tetsu-comp to use it`), a card that
  goes away takes its outputs with it, and losing the render device
  cannot be survived. The render device is not chosen again when a monitor
  is plugged.

  **Dmabuf and explicit sync.** `zwp_linux_dmabuf_v1` is advertised with
  default and per-output feedback; `wl_drm` is not. With
  `wp_linux_drm_syncobj_manager_v1` a client's acquire point holds its
  commit back until it signals, else the dmabuf's implicit fence does.
  Explicit sync is on when the render device supports `syncobj_eventfd`
  and, on NVIDIA, the driver is 555 or newer; otherwise the log says why
  (`tetsu-comp: explicit sync disabled (no syncobj_eventfd)`).

- **`--headless-outputs <n>`** (headless only, 1 to 4): adds
  `HEADLESS-2`.. to the right of the first output. It exists so that tests
  can exercise the `output` window rule.

- **`--headless-hotplug`** (headless only): reads hotplug commands from
  stdin, one per line, so tests can plug and unplug outputs:
  `add [<w>x<h>]` (a new `HEADLESS-<n>`, `n` counting up),
  `add-named <name> <w>x<h>` (a replug) and `remove <name>`. Sides are 1 to
  16384. A line that does not parse, or names nothing, is logged and
  ignored.

Query the bus with `varlinkctl call $TETSU_COMP_SOCKET
org.tetsu.comp.window.v1.ListWindows '{}'`. The bus socket is
`$XDG_RUNTIME_DIR/tetsu/comp/<wayland socket name>`, one per instance.

## Outputs

Outputs appear and disappear while the compositor runs (udev on the tty, the
stdin hook headless). `org.tetsu.comp.output.v1` serves `ListOutputs` and
`WatchOutputs`: modes, current mode, scale, transform, position and the EDID
make (the three-letter PNP id, for example `GSM`), model and serial. The EDID
comes from the display, so control characters are stripped and each field is
cut to 32 characters. `ConfigureOutputs` answers
`org.varlink.service.MethodNotImplemented` until #97.

- **Ids.** `out-<n>`, never reused: a monitor plugged in again has a new id.
  Follow it by `name` or by the EDID fields.
- **Layout.** Enabled outputs are packed left to right at `y = 0`, in the
  order they were added, by logical width (mode over scale, swapped for 90
  and 270 degrees). They are packed again on every add, remove or mode
  change, so removing an output moves the ones to its right.
- **Unplug.** The workspaces of the output move to the first remaining
  output, after its highest index and in their order; each remembers its
  home (connector name, index, whether it was active). When an output with
  that name comes back, they return to their old indices and the one that
  was active is active again; no workspace is created. With no output
  left, workspaces have `output_id: null` and nothing is focused; the next
  output adopts them. An output with a new name gets a fresh workspace.
  `Super+N` does not reach a moved workspace whose index is above 8 until
  its home returns.
- **`wl_output`.** The global is disabled at once and destroyed 10 s later.

### Layer shell

`zwlr_layer_shell_v1` is Smithay's `LayerMap`. A layer surface without a
`wl_output` goes on the focused output. Exclusive zones narrow the layout
viewport (`layout::usable`), so a bar on top makes the tiles start below it.
Layers get frame callbacks and pointer input (overlay, top, windows, bottom,
background). A mapped `exclusive` layer on `top` or `overlay` takes the
keyboard; `on_demand` gets none yet. When an output goes away its layer
surfaces get `closed`: a client must make them again when an output appears
(tetsu-bar does).

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

    [render]
    device                    = "auto"  # or "/dev/dri/renderD129" (a node under /dev/dri/)
    explicit_sync             = "auto"  # auto | on | off; "on" cannot force an unsupported device
    wait_for_frame_completion = false   # wait for each frame before queueing it

A window that opens on another workspace or output than the focused one
does not take the focus. Rules are evaluated at the first configure and at
map, not on later title changes.

## Modules

- `model`: windows, workspaces, outputs and the events they emit, in the
  order the contract demands. Pure, tested first. `model/outputs.rs` keeps
  the output state and its layout.
- `outputs/`: EDID parsing, mode lists and logical geometry. Pure.
- `layout/`: the algorithms (`dwindle`, `master`, `floating`). Pure.
- `rules`: app_id and title patterns to a decision. Pure.
- `settings`: the config file.
- `gpu/`: the tty backend's decisions, pure and tested first. `select`
  (the render device), `assign` (how each output is served), `formats`
  (CCS filter, color formats, scanout tranches) and `sync` (explicit sync
  and how a commit is gated).
- `engine`: the model plus per-workspace layout state; `apply(Action)`
  and `arrange` give events and rects. Pure.
- `grab`: the pointer grab that moves a floating window.
- `bus`: the bus thread. The compositor loop sends deltas over an
  unbounded channel and never waits on it; a watcher that falls 256 events
  behind is disconnected, and a dead thread is restarted.
- `state`, `handlers`, `input`, `render`, `backend`: the Smithay side.
  `handlers/layer_shell.rs` is the layer-shell glue.

## Credits

`gpu/` and `backend/udev/` port the multi-GPU policy of niri
(`src/backend/tty.rs` at commit `fcbc971cf49a987b1e8d5ff1d64da6b1f61a1787`,
GPL-3.0-or-later, like this crate): the render node probe, display-only
devices, the CCS modifier filter, the color formats, the per-output dmabuf
feedback, the NVIDIA >= 555 rule and the frame flags. The `MultiRenderer`
plumbing follows Smithay's `anvil` (MIT).
