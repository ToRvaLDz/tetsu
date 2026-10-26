//! The unit in `systemd/` carries what the code assumes.

const UNIT: &str = include_str!("../../systemd/tetsu-boot-check.service");

#[test]
fn the_unit_gates_boot_complete() {
    for line in [
        "After=multi-user.target",
        "Before=boot-complete.target",
        "Type=oneshot",
        "RemainAfterExit=yes",
        "ExecStart=/usr/bin/tetsu-boot-check",
        "WantedBy=multi-user.target",
        "RequiredBy=boot-complete.target",
    ] {
        assert!(UNIT.lines().any(|l| l == line), "{line}");
    }
}
