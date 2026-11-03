# tetsu-secrets

The secrets vault of Tetsu (decision #25, build ticket #58). One broker,
`tetsu-secrets serve`, is the only process that opens the vault; everything
else talks to it over `org.tetsu.secrets.v1`.

## Layout

| What | Where |
|---|---|
| Vault (oo7 v1 keyring, 0600, dir 0700) | `~/.local/share/tetsu/secrets/vault.keyring` |
| Sealed vault key | `$XDG_CONFIG_HOME/credstore.encrypted/tetsu-secrets.vault-key` (default `~/.config`) |
| Socket (owned by `tetsu-secrets.socket`) | `$XDG_RUNTIME_DIR/tetsu/org.tetsu.secrets.v1` |
| Staged installer key (imported once, then wiped) | `~/.local/share/tetsu/staged-api-key` |

## Units (`dist/`)

- `tetsu-secrets-init.service` runs once, at the first login, when the
  sealed key does not exist: `tetsu-secrets init` makes 32 random bytes,
  pipes them to `systemd-creds encrypt --user` and never replaces a key
  or creates one beside a vault that has content.
- `tetsu-secrets.socket` listens on the socket (0600, directory 0700) and
  is wanted by the user `sockets.target`, which `default.target` pulls in:
  it is up from the start of the user manager, with no login session
  needed. The image preset enables it for every user.
- `tetsu-secrets.service` is started by the first connection. It has
  `LoadCredentialEncrypted=`: systemd decrypts the key into
  `$CREDENTIALS_DIRECTORY` and the broker unlocks the vault from it. The
  broker takes the listening socket from systemd (`LISTEN_PID`,
  `LISTEN_FDS`, fd 3); started by hand it binds the path itself.

`--seal auto` (the default) binds the key to the host and, when there is
one, the TPM. `--seal host` binds it to the host key only.

## The bus

No call returns a value. `Add`, `Rotate` and `Remove` take the value as a
`@sensitive` input; the agent unit gets `PermissionDenied` on all three.
Replies and errors name ids and parameters, never values, and the facade
never logs or traces message bodies. Usage counters (`use_count`,
`last_used_at`) live in memory since the broker started.

A value leaves the broker only through `Broker::use_secret`, for code in
the same process (the HTTP proxy and the spawn code to come): a proxied
use must name exactly the bound host, a spawn a tool id in
`allowed_tools`.

## Audit

Every access is written to the journal before it happens, over the native
journald protocol, and the access is refused when the write fails. Find
the entries with:

    journalctl --user -t tetsu-secrets

Fields: `SYSLOG_IDENTIFIER=tetsu-secrets`, a fixed `MESSAGE_ID` per
action (`add`, `remove`, `rotate`, `use`, `spawn`, `denied`, `unlock`,
`list`), `TETSU_ACTION`, `TETSU_OUTCOME` (`ok`, `failed`, `denied`),
`TETSU_CALLER_UNIT`, `TETSU_CALLER_PID`, `TETSU_CALLER_EXE`,
`TETSU_CALLER_COMM`, and when they apply `TETSU_SECRET_ID`,
`TETSU_TOOL_ID`, `TETSU_UPSTREAM`, `TETSU_HTTP_STATUS`, `TETSU_REASON`.
No field can hold a secret value.

For tests, `TETSU_SECRETS_JOURNAL_SOCKET` replaces the journal socket the
broker writes to.

## Human check (not run by the test suite)

With a scratch `HOME`:

1. `HOME=<scratch> tetsu-secrets init`
2. `systemd-run --user --wait --pipe -p LoadCredentialEncrypted=tetsu-secrets.vault-key:<scratch>/.config/credstore.encrypted/tetsu-secrets.vault-key -E HOME=<scratch> tetsu-secrets serve`
3. Add a secret, restart the broker, list it again.
4. `journalctl --user -t tetsu-secrets` shows `unlock`, `add` and `list`.

In the VM, install the image and check that
`systemctl --user status tetsu-secrets.socket` is active (listening) as
soon as the user manager runs, also with no login session (`loginctl
enable-linger` and no login). Connect to the socket, then check that
`tetsu-secrets-init` and `tetsu-secrets` are both successful, that the
credential exists, and that the vault unlocks again after a reboot with
no prompt.
