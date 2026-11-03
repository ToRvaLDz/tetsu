#!/usr/bin/env bash
# Bus access policy checks of the installed system (sourced by guest.sh):
# the registries, the tx socket unit and the polkit actions and rules.
# Expectations come from the IDL on the host, never from the image.

# The polkit action ids the IDL asks for, one per line, sorted.
expected_polkit_actions() {
  grep -h '^# @capability ' "$REPO"/crates/tetsu-bus/idl/*.varlink \
    | awk '{print $3}' | sort -u | sed 's/_/-/g; s/^/org.tetsu./'
}

bus_policy_checks() {
  local want got
  # System registry.
  check "/run/tetsu and its registry are 0755 root directories" '^755 root directory$' \
    "stat -c '%a %U %F' /run/tetsu /run/tetsu/registry | sort -u"
  check "the registry links the tx socket" '^/run/tetsu/org\.tetsu\.tx\.v1$' \
    "readlink /run/tetsu/registry/org.tetsu.tx.v1"
  check "the tx socket is 0666 root" '^666 root socket$' \
    "stat -L -c '%a %U %F' /run/tetsu/registry/org.tetsu.tx.v1"
  check "tetsu-tx.socket is active" '^active$' "systemctl is-active tetsu-tx.socket"
  check "varlinkctl reaches org.tetsu.tx.v1 through the registry (activation)" 'org\.tetsu\.tx\.v1' \
    "varlinkctl info /run/tetsu/registry/org.tetsu.tx.v1"
  check "a user cannot add to the registry" 'Permission denied' \
    "ln -s /tmp/x /run/tetsu/registry/fake 2>&1"
  # Session registry.
  check "the session directories are 0700 of the user" "^700 $USER_NAME directory\$" \
    "stat -c '%a %U %F' \"\$XDG_RUNTIME_DIR/tetsu\" \"\$XDG_RUNTIME_DIR/tetsu/registry\" | sort -u"
  # polkit.
  want=$(expected_polkit_actions)
  gexec "pkaction | grep '^org\\.tetsu\\.' | sort | tr -d '\\r'"
  got=$GOUT
  assert_eq "$SCENARIO: pkaction lists one action per capability class" "$want" "$got"
  check "polkit vendor rules.d is 0755 root:root, as the package ships it" '^755 root:root$' \
    "sudo -n stat -c '%a %U:%G' /usr/share/polkit-1/rules.d"
  check "polkit admin rules.d is 0750 root:polkitd" '^750 root:polkitd$' \
    "sudo -n stat -c '%a %U:%G' /etc/polkit-1/rules.d"
  check_not "polkitd loaded the rules without error" 'Error (compiling|evaluating|loading)|SyntaxError|Error on line' \
    "sudo -n journalctl -b -u polkit --no-pager"
  # A wheel user gets YES in a local active session, a challenge (2)
  # otherwise. polkit calls a session local only when it has a seat, so a
  # serial-console login (no seat) is expected to be challenged.
  check "pkcheck for the wheel user matches the session state" '^ok$' \
    "a=\$(loginctl show-session \"\$XDG_SESSION_ID\" -p Active --value); seat=\$(loginctl show-session \"\$XDG_SESSION_ID\" -p Seat --value); out=\$(pkcheck --action-id org.tetsu.packages.install --process \$\$ 2>&1); rc=\$?; if [ \"\$a\" = yes ] && [ -n \"\$seat\" ]; then want=0; else want=2; fi; [ \$rc -eq \$want ] && echo ok || echo \"active=\$a seat=\$seat rc=\$rc want=\$want out=\$out\""
}
