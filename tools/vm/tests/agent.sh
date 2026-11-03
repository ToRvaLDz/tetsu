#!/usr/bin/env bash
# Sandbox checks of tetsu-agent.service in the installed system (sourced by
# guest.sh; called by session.sh once the session is up). #82.
#
# A runtime drop-in swaps only ExecStart= of the shipped unit for
# agent-probe.sh, so the probes run under the exact shipped sandbox; the same
# probe also runs as a plain user service, the positive control. Every
# probe is an operation whose failure is asserted, with the control in the
# same check. The probe prints P:<name>=<value> lines; the unit's lines are
# read from the journal.

AGENT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# Where the probe and its files live in the guest (expanded by the guest shell).
AGENT_PDIR='$HOME/.cache/tetsu-vm-probe'
# Open items reported next to the summary, never counted as passes.
AGENT_KNOWN_OPEN=()

# gput LOCAL REMOTE MODE: copy a file to the guest through the serial
# console (base64 in short lines), and compare the checksum.
gput() {
  local b64 chunk sum
  b64=$(base64 -w0 "$1")
  sum=$(sha256sum "$1" | awk '{print $1}')
  gexec "mkdir -p \"\$(dirname $2)\"; : >$2.b64"
  while [ -n "$b64" ]; do
    chunk=${b64:0:1200}
    b64=${b64:1200}
    gexec "printf %s $chunk >>$2.b64"
  done
  gexec "base64 -d $2.b64 >$2 && rm -f $2.b64 && chmod $3 $2 && sha256sum $2 | awk '{print \$1}'"
  assert_eq "$SCENARIO: $(basename "$1") reached the guest intact" "$sum" "$(tail -n 1 <<<"$GOUT" | tr -d '\r')"
}

# The clone3 probe: no compiler in the guest and no perl in the base set, so
# a 142-byte static x86-64 ELF that returns the errno of clone3(NULL, 0).
agent_clone3_probe() {
  printf '\x7fELF\x02\x01\x01\x00\x00\x00\x00\x00\x00\x00\x00\x00\x02\x00\x3e\x00\x01\x00\x00\x00\x78\x00\x40\x00\x00\x00\x00\x00\x40\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x40\x00\x38\x00\x01\x00\x00\x00\x00\x00\x00\x00\x01\x00\x00\x00\x05\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x40\x00\x00\x00\x00\x00\x00\x00\x40\x00\x00\x00\x00\x00\x8e\x00\x00\x00\x00\x00\x00\x00\x8e\x00\x00\x00\x00\x00\x00\x00\x00\x10\x00\x00\x00\x00\x00\x00\xb8\xb3\x01\x00\x00\x31\xff\x31\xf6\x0f\x05\x89\xc7\xf7\xdf\xb8\x3c\x00\x00\x00\x0f\x05' >"$1"
}

# agent_parse VARNAME TEXT: P:name=value lines into the associative array.
agent_parse() {
  local -n arr=$1
  local line
  arr=()
  while IFS= read -r line; do
    line=${line%$'\r'}
    if [[ $line =~ ^P:([a-z0-9_]+)=(.*)$ ]]; then arr[${BASH_REMATCH[1]}]=${BASH_REMATCH[2]}; fi
  done <<<"$2"
}

# agent_expect NAME INSIDE OUTSIDE DESCRIPTION: INSIDE is what the sandboxed
# probe must see, OUTSIDE what the control must see (globs; - for any).
declare -A AGENT_IN AGENT_OUT
agent_expect() {
  local name=$1 in_want=$2 out_want=$3 desc=$4 in_got out_got ok=1
  in_got=${AGENT_IN[$name]-<missing>}
  out_got=${AGENT_OUT[$name]-<missing>}
  # shellcheck disable=SC2053
  [ "$in_want" = - ] || [[ $in_got == $in_want ]] || ok=0
  # shellcheck disable=SC2053
  [ "$out_want" = - ] || [[ $out_got == $out_want ]] || ok=0
  if [ "$ok" = 1 ]; then
    pass "$SCENARIO: $desc"
    evidence "$name: inside=$in_got control=$out_got"
  else
    fail "$SCENARIO: $desc" "$name: inside=$in_got (want $in_want), control=$out_got (want $out_want)"
  fi
}

# agent_journal INVOCATION: the P: lines of one invocation of the unit.
agent_journal() {
  gexec "journalctl --user _SYSTEMD_INVOCATION_ID=$1 -o cat --no-pager | grep '^P:'" 60
}

agent_known_open_summary() {
  local k
  [ "${#AGENT_KNOWN_OPEN[@]}" -gt 0 ] || return 0
  printf '\nknown open (reported, not counted as passes):\n'
  for k in "${AGENT_KNOWN_OPEN[@]}"; do printf '  KNOWN-OPEN %s\n' "$k"; done
}

agent_sandbox_checks() {
  local inv elf="$WORK/clone3-probe" pd=$AGENT_PDIR

  # 1. What is installed: a plain unit, no preset, the denied directories made.
  check "tetsu-agent.service is installed and not enabled by any preset" '^static$' \
    "systemctl --user is-enabled tetsu-agent.service"
  check "the unit runs in tetsu-agent.slice with Type=exec" 'Slice=tetsu-agent\.slice' \
    "systemctl --user show -p Slice -p Type tetsu-agent.service"
  check "the denied home directories exist, 0700, made by user tmpfiles" '^700$' \
    "stat -c %a ~/.config/credstore ~/.config/credstore.encrypted ~/.local/lib/credstore ~/.local/lib/credstore.encrypted ~/.local/share/tetsu ~/.local/share/keyrings | sort -u"

  # 2. Setup outside the sandbox: probe, markers, sibling cgroup.
  agent_clone3_probe "$elf"
  gput "$AGENT_DIR/agent-probe.sh" "$pd/probe.sh" 0755
  gput "$elf" "$pd/clone3" 0755
  gexec "for d in ~/.config/credstore ~/.config/credstore.encrypted ~/.local/lib/credstore ~/.local/lib/credstore.encrypted ~/.local/share/tetsu ~/.local/share/keyrings ~/.config/tetsu ~/.local/state/tetsu; do mkdir -p \$d && printf vm-marker >\$d/vm-marker; done; echo markers-seeded"
  assert_contains "$SCENARIO: markers are seeded in the denied and the read-only locations" "$GOUT" "markers-seeded"
  check "a sibling cgroup is made outside the agent slice, in the user's delegated subtree" '^sibling-ok$' \
    "ucg=/sys/fs/cgroup\$(systemctl show -P ControlGroup user@\$(id -u).service); mkdir -p \$ucg/vm-probe-sibling && test -w \$ucg/vm-probe-sibling/cgroup.procs && echo sibling-ok"
  gexec "printf 'SIB=%s\\nGO=%s\\nCLONE3=%s\\nCAPD_PID=%s\\n' \"/sys/fs/cgroup\$(systemctl show -P ControlGroup user@\$(id -u).service)/vm-probe-sibling\" \"$pd/go\" \"$pd/clone3\" \"\$(systemctl --user show -P MainPID tetsu-capd.service)\" >$pd/probe.env; rm -f $pd/go; cat $pd/probe.env"
  assert_contains "$SCENARIO: the probe environment names capd's pid" "$GOUT" "CAPD_PID="
  grep -qE '^CAPD_PID=[1-9][0-9]*[[:space:]]*$' <<<"$GOUT" \
    || fail "$SCENARIO: CAPD_PID is a non-empty number (capd is running)" "$(grep '^CAPD_PID=' <<<"$GOUT")"

  # 3. Inside: the shipped unit with only ExecStart= swapped.
  gexec "d=\$XDG_RUNTIME_DIR/systemd/user/tetsu-agent.service.d; mkdir -p \$d && printf '[Service]\\nExecStart=\\nExecStart=/usr/bin/bash %s in %s\\n' \"$pd/probe.sh\" \"$pd/probe.env\" >\$d/zz-vm-probe.conf && systemctl --user daemon-reload && systemctl --user start --no-block tetsu-agent.service && echo started"
  assert_contains "$SCENARIO: the probe unit is started" "$GOUT" "started"
  gexec "$(guest_poll 60 '[ -n "$(systemctl --user show -P InvocationID tetsu-agent.service)" ]'); systemctl --user show -P InvocationID tetsu-agent.service"
  inv=$(tail -n 1 <<<"$GOUT" | tr -dc '0-9a-f')
  [ -n "$inv" ] || die "the probe unit has no invocation"
  gexec "$(guest_poll 120 "journalctl --user _SYSTEMD_INVOCATION_ID=$inv -o cat --no-pager | grep -q '^P:late_ready'"); journalctl --user _SYSTEMD_INVOCATION_ID=$inv -o cat --no-pager | tail -n 5" 150
  assert_contains "$SCENARIO: the sandboxed probe ran up to the late-socket step" "$GOUT" "P:"

  # A socket made in the runtime dir after the unit started.
  gexec "(systemd-socket-activate -l \"\$XDG_RUNTIME_DIR/vm-late.sock\" sleep 300 >/dev/null 2>&1 &); $(guest_poll 20 'test -S "$XDG_RUNTIME_DIR/vm-late.sock"'); test -S \"\$XDG_RUNTIME_DIR/vm-late.sock\" && echo late-made; touch $pd/go"
  assert_contains "$SCENARIO: a socket is made in the runtime dir after the unit started" "$GOUT" "late-made"
  gexec "$(guest_poll 150 "journalctl --user _SYSTEMD_INVOCATION_ID=$inv -o cat --no-pager | grep -q '^P:done=1'"); systemctl --user is-active tetsu-agent.service" 180
  agent_journal "$inv"
  agent_parse AGENT_IN "$GOUT"

  # 4. Outside: the same probe as a plain user service, the control.
  gexec "systemd-run --user --pipe --wait --collect -q /usr/bin/bash $pd/probe.sh out $pd/probe.env | grep '^P:'" 240
  agent_parse AGENT_OUT "$GOUT"
  [ "${#AGENT_IN[@]}" -gt 10 ] && [ "${#AGENT_OUT[@]}" -gt 10 ] \
    || fail "$SCENARIO: both probes printed their results" "inside=${#AGENT_IN[@]} lines, control=${#AGENT_OUT[@]}"
  evidence "inside: ${#AGENT_IN[@]} probes, control: ${#AGENT_OUT[@]} probes"

  # 5. Assertions: inside / control.
  agent_expect tools_rc 0 0 "tetsu-agent tools --json exits 0 inside the sandbox"
  agent_expect tools_cap yes yes "inside, capd is reached and verified through the read-only %t/tetsu"
  agent_expect tools_notify yes yes "inside, notifyd is reached and verified through the read-only %t/tetsu"
  agent_expect tools_left_out no no "inside, neither daemon is reported as left out"
  agent_expect w_statedir yes - "the agent writes its own StateDirectory"
  agent_expect w_config no yes "creating a file in ~/.config/tetsu fails"
  agent_expect w_state no yes "creating a file in ~/.local/state/tetsu fails"
  agent_expect w_runtime no yes "creating a file in %t/tetsu fails"
  agent_expect r_config yes yes "the ~/.config/tetsu marker is still readable"
  agent_expect r_state yes yes "the ~/.local/state/tetsu marker is still readable"
  agent_expect r_runtime yes yes "%t/tetsu is still listable"
  agent_expect r_cred_cfg no yes "the config credstore marker cannot be read"
  agent_expect r_cred_cfg_enc no yes "the config credstore.encrypted marker cannot be read"
  agent_expect r_cred_lib no yes "the ~/.local/lib/credstore marker cannot be read"
  agent_expect r_cred_lib_enc no yes "the ~/.local/lib/credstore.encrypted marker cannot be read"
  agent_expect r_vault no yes "the marker in ~/.local/share/tetsu (vault, staged key) cannot be read"
  agent_expect r_keyring no yes "the keyring marker cannot be read"
  agent_expect c_userbus no yes "busctl --user cannot reach the session bus"
  agent_expect c_userbus_path no yes "connecting to %t/bus fails"
  agent_expect c_sysbus no yes "busctl --system cannot reach the system bus"
  agent_expect c_sysbus_path no yes "connecting to /run/dbus/system_bus_socket fails"
  agent_expect c_usermgr no yes "connecting to %t/systemd/private fails"
  agent_expect c_cred no yes "connecting to /run/systemd/io.systemd.Credentials fails"
  agent_expect c_mgr no yes "connecting to /run/systemd/io.systemd.Manager fails"
  agent_expect c_late no yes "a socket made in %t after the unit started cannot be connected"
  agent_expect late_visible no yes "a socket made in %t after the unit started is not visible"
  if [ "${AGENT_OUT[c_oo7_pam]-no}" = yes ]; then
    agent_expect c_oo7_pam no yes "oo7's pam.sock cannot be reached"
  else
    say_info "oo7-daemon is not running in the guest: pam.sock is covered by the %t allowlist only"
  fi
  agent_expect dns yes yes "name resolution works inside"
  agent_expect systemctl_user no yes "systemctl --user fails inside"
  agent_expect systemd_run_user no yes "systemd-run --user fails inside"
  agent_expect tpm no yes "/dev/tpmrm0 is absent inside and present outside"
  agent_expect dbus_env unset - "DBUS_SESSION_BUS_ADDRESS is unset inside"
  agent_expect abstract_bus no - "no @-abstract bus address exists in /proc/net/unix"
  agent_expect cg_self yes no "the probe's /proc/self/cgroup is in tetsu-agent.slice"
  agent_expect cg_child yes no "a forked child's cgroup is in tetsu-agent.slice"
  agent_expect cg_join no yes "writing the probe's pid to a sibling cgroup's cgroup.procs fails"
  agent_expect clone3 enosys 'errno-*' "clone3 returns ENOSYS inside and not outside"
  agent_expect userns yes no "the probe runs in a user namespace"
  agent_expect m_mount no no "mount fails inside"
  agent_expect m_umount no no "umount fails inside"
  agent_expect m_unshare no yes "unshare of a user namespace fails inside (RestrictNamespaces)"
  agent_expect m_unshare_mount no yes "mounting in a fresh namespace fails inside"
  # The empty bounding set, ~@mount and RestrictNamespaces are covered together
  # by the mount, unshare and unshare-mount probes above, not each in isolation.
  # Known open item: not a pass, not a failure. It is reported only when the
  # control can do the read; otherwise the probe proves nothing and the check fails.
  if [ "${AGENT_OUT[proc_root]-<missing>}" = yes ]; then
    AGENT_KNOWN_OPEN+=("/proc route (#144): reading /proc/<capd pid>/root/ from inside: inside=${AGENT_IN[proc_root]-<missing>} control=yes")
    say_info "KNOWN-OPEN /proc route (#144): inside=${AGENT_IN[proc_root]-<missing>} control=yes"
  else
    fail "$SCENARIO: the /proc/<capd pid>/root control can read it outside" "proc_root: control=${AGENT_OUT[proc_root]-<missing>} (want yes), inside=${AGENT_IN[proc_root]-<missing>}"
  fi

  # 6. Back to the shipped unit, unmodified.
  gexec "pkill -u \$(id -u) -f 'vm-late.sock|sleep 300'; rm -f \$XDG_RUNTIME_DIR/vm-late.sock; rm -rf \$XDG_RUNTIME_DIR/systemd/user/tetsu-agent.service.d; ucg=/sys/fs/cgroup\$(systemctl show -P ControlGroup user@\$(id -u).service); rmdir \$ucg/vm-probe-sibling; for d in ~/.config/credstore ~/.config/credstore.encrypted ~/.local/lib/credstore ~/.local/lib/credstore.encrypted ~/.local/share/tetsu ~/.local/share/keyrings ~/.config/tetsu ~/.local/state/tetsu; do rm -f \$d/vm-marker; done; rm -rf $pd; systemctl --user daemon-reload; systemctl --user reset-failed tetsu-agent.service; systemctl --user start tetsu-agent.service; echo restored"
  assert_contains "$SCENARIO: the unmodified unit starts" "$GOUT" "restored"
  check "the unmodified unit runs /usr/bin/tetsu-agent" 'path=/usr/bin/tetsu-agent' \
    "systemctl --user show -P ExecStart tetsu-agent.service"
  check "the unmodified unit exited 0" '^ExecMainStatus=0$' \
    "systemctl --user show -p ExecMainStatus tetsu-agent.service"
  # The unit is a one-shot that exits 0 at once and is then garbage-collected,
  # so its InvocationID is empty by now: look the line up by unit and
  # identifier. journald stamps _SYSTEMD_CGROUP from the live process, which
  # is the real placement of the unmodified unit.
  check "the unit's journal line is in tetsu-agent.slice" '^_SYSTEMD_CGROUP=.*/tetsu-agent\.slice/' \
    "$(guest_poll 30 'journalctl --user _SYSTEMD_USER_UNIT=tetsu-agent.service SYSLOG_IDENTIFIER=tetsu-agent -o cat --no-pager | grep -q "only the rules act"'); journalctl --user _SYSTEMD_USER_UNIT=tetsu-agent.service SYSLOG_IDENTIFIER=tetsu-agent -o verbose --no-pager | grep '^ *_SYSTEMD_CGROUP=' | sed 's/^ *//' | tail -n 1"
}
