# tetsu-capd

The capability daemon. It serves `org.tetsu.cap.v1` at
`$XDG_RUNTIME_DIR/tetsu/org.tetsu.cap.v1` (a 0600 socket in a 0700
directory) and decides, for every call of the agent that a Tetsu daemon
hands it through `Check`, whether the call may run.

## Grants

Each capability class (the `@capability` tags of the bus IDL, for example
`packages.install`) is in one state:

| state       | a call of the class                                   |
|-------------|-------------------------------------------------------|
| `unset`     | asks, with *Allow once*, *Always allow*, *Deny*       |
| `ask`       | asks, with *Approve*, *Deny*                          |
| `notify`    | runs; the user is told once per 10 s per class        |
| `automatic` | runs                                                  |
| `off`       | is refused                                            |

A method tagged `@destructive` asks whatever the state, except `off`,
which refuses first. A *Deny* (or a dismissed ask) holds back further asks
of that class for 24 hours; they are denied with `denied_by: suppressed`.
If the notification service cannot be reached the call is denied
(`unavailable`) and nothing is suppressed.

## The three layers

| layer     | file                                             | written by |
|-----------|--------------------------------------------------|------------|
| `image`   | `/usr/share/tetsu/capabilities.toml`             | the image  |
| `managed` | `$XDG_STATE_HOME/tetsu/capabilities.toml`        | `tetsu cap`, *Always allow* |
| `user`    | `$XDG_CONFIG_HOME/tetsu/capabilities.toml`       | you        |

The nearest layer wins, key by key: user over managed over image. The
agent can change only the managed layer, and the daemon refuses `Set` for
the agent unit. A key set in the user file cannot be changed through
`Set` (`OverriddenByUser`).

```toml
[class]
"packages.install" = "notify"
"system.update" = "ask"

[scope."mail.send"]
work = "ask"
```

Values are `automatic`, `notify`, `ask` or `off`. A scoped key, when any
layer sets it, wins over the class key. Classes that no method has are
ignored and named once on stderr. The files are read again at every call,
so a hand edit takes effect at once.

A layer that does not parse fails closed: `Check` denies with
`unavailable`, `Set` is refused, and the file and line are logged. `tetsu
cap list` keeps answering from the last good parse.

## Audit

Every decision is written to the journal with `SYSLOG_IDENTIFIER=tetsu-capd`
and one `MESSAGE_ID` per event (`check`, `ask`, `approved`, `denied`,
`set`, `suppressed`). If the journal cannot be written, an allow becomes a
deny and `Set` is refused.

    journalctl --user -t tetsu-capd -o verbose

## Environment

| variable                    | meaning                                    |
|-----------------------------|--------------------------------------------|
| `XDG_RUNTIME_DIR`           | required                                   |
| `XDG_STATE_HOME`, `XDG_CONFIG_HOME` | default `~/.local/state`, `~/.config` |
| `TETSU_CAPD_IMAGE_FILE`     | the image layer (default as in the table)  |
| `TETSU_CAPD_JOURNAL_SOCKET` | the journal socket, for tests              |

## Running it from the source tree

Start the notification server, then the daemon:

    tools/uat/notifyd-up.sh
    cargo run -p tetsu-capd

and, from a unit that `Check` accepts (`tetsu-comp`, `tetsu-notifyd`,
`tetsu-secrets` in the user manager, or tetsu-tx as root):

    systemd-run --user --unit=tetsu-comp --pipe --wait \
      varlinkctl call unix:$XDG_RUNTIME_DIR/tetsu/org.tetsu.cap.v1 \
      org.tetsu.cap.v1.Check \
      '{"peer":{"pid":1,"uid":1000,"unit":"tetsu-agent.service"},"method":"org.tetsu.tx.v1.Install","args":{"packages":["firefox"]}}'

## Installed files

- `/usr/bin/tetsu-capd`
- `/usr/lib/systemd/user/tetsu-capd.service` (`dist/tetsu-capd.service`)
- `/usr/share/tetsu/capabilities.toml` (`dist/capabilities.toml`)
