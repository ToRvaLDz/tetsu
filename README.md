# Tetsu

Tetsu is an immutable, Arch-based, AI-native Linux distribution. Its system services are Rust daemons talking over a Varlink bus, and it ships its own Smithay-based Wayland compositor.

The full (Italian) specification is in [specification](Distro%20Linux%20AI-native%20%E2%80%94%20Spec%20iniziale.md).

## Layout

- `crates/tetsu-bus`: Varlink interface definitions and the bus contract
- `crates/tetsu-tx`: transactional system updates
- `crates/tetsu-comp`: the Smithay compositor
- `crates/tetsu-bar`: the status bar
- `crates/tetsu-notifyd`: the notification server
- `crates/tetsu-agent`: the AI agent daemon
- `crates/tetsu-secrets`: the secrets daemon
- `image/`: the mkosi image build pipeline (placeholder)

## License

MIT OR Apache-2.0 (see `LICENSE`, `LICENSE-MIT`, `LICENSE-APACHE`), except `crates/tetsu-comp`, which is GPL-3.0-or-later.
