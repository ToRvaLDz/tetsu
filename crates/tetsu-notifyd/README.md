# tetsu-notifyd

The Tetsu notification server, as decided in
[Decide who serves notifications and their actions](https://github.com/ToRvaLDz/tetsu/issues/23).

- **`org.tetsu.notify.v1`** (Varlink) at `$XDG_RUNTIME_DIR/tetsu/org.tetsu.notify.v1`,
  for bus callers. An action answers only its caller, only from a pointer
  click in notifyd's own surfaces, never while the session is locked and
  never before its buttons arm (about 500 ms). A caller that disconnects
  withdraws its notification. Critical notifications never expire: their
  popup collapses into the panel after 30 s, still pending.
- **`org.freedesktop.Notifications`** (D-Bus, spec 1.3) for applications:
  capabilities `actions`, `body`, `icon-static`, `persistence`. Markup is
  stripped to plain text.
- **Popups and the panel** on layer-shell (SCTK, tiny-skia, cosmic-text).
  Bus callers get the Tetsu frame naming the sender from its socket
  credentials; freedesktop notifications never do. Popups never take the
  keyboard; the panel takes it on demand and handles Esc only. History is
  in memory only, the last 50 notifications.

## Lock state

`TETSU_COMP_SOCKET` names the `tetsu-comp` socket serving
`org.tetsu.comp.session.v1`. When it is unset (a foreign compositor during
development) the session counts as unlocked and notifyd says so once on
stderr. When it is set but unreachable, the session counts as locked until
the compositor answers.

## Running it in development

Your session most likely has a notification server already. To try notifyd
next to it without replacing it, give it a private D-Bus daemon:

```bash
eval "$(dbus-launch --sh-syntax)"
cargo run -p tetsu-notifyd
```

Then, from a shell with the same `DBUS_SESSION_BUS_ADDRESS`:

```bash
notify-send -a demo "Hello" "From notify-send"
```

```bash
varlinkctl call --more unix:$XDG_RUNTIME_DIR/tetsu/org.tetsu.notify.v1 \
  org.tetsu.notify.v1.Show \
  '{"title":"Install firefox?","urgency":"critical","actions":[{"id":"approve","label":"Approve"},{"id":"deny","label":"Deny"}]}'
```

`varlinkctl` prints `shown`, then `invoked` or `dismissed` once you click;
stopping it withdraws the notification. `--replace` takes
`org.freedesktop.Notifications` from a running server on the real session
bus.

## Installing

- `dist/tetsu-notifyd.service` → `/usr/lib/systemd/user/`
- `dist/org.freedesktop.Notifications.service` → `/usr/share/dbus-1/services/`
