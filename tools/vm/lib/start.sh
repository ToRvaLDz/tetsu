#!/usr/bin/env bash
# `tetsu-vm start`: argument parsing, QEMU argv construction, launch.

OVMF_CODE_CANDIDATES=(
  /usr/share/edk2/x64/OVMF_CODE.secboot.4m.fd
  /usr/share/OVMF/OVMF_CODE_4M.secboot.fd
  /usr/share/edk2/ovmf/OVMF_CODE.secboot.fd
)
OVMF_VARS_CANDIDATES=(
  /usr/share/edk2/x64/OVMF_VARS.4m.fd
  /usr/share/OVMF/OVMF_VARS_4M.fd
  /usr/share/edk2/ovmf/OVMF_VARS.fd
)

# First existing path among the candidates, or the override when set.
find_firmware() {
  local override=$1 label=$2 f
  shift 2
  if [ -n "$override" ]; then
    [ -r "$override" ] || die "$label override '$override' is not readable"
    printf '%s' "$override"
    return
  fi
  for f in "$@"; do
    [ -r "$f" ] && { printf '%s' "$f"; return; }
  done
  die "no $label found; install OVMF (edk2) or set the TETSU_OVMF_* variable"
}

# Double the commas, as QEMU option values require.
qesc() { printf '%s' "${1//,/,,}"; }

kvm_usable() {
  case "${TETSU_VM_KVM:-auto}" in
    1) return 0 ;;
    0) return 1 ;;
  esac
  [ -r /dev/kvm ] && [ -w /dev/kvm ]
}

# Print ARGV on one line. Commas stay literal (QEMU syntax); only arguments
# holding whitespace or quotes are single-quoted.
print_argv() {
  local a out=""
  for a in "${ARGV[@]}"; do
    case "$a" in
      *[[:space:]\'\"]*) a="'${a//\'/\'\\\'\'}'" ;;
    esac
    out+="${out:+ }$a"
  done
  printf '%s\n' "$out"
}

start_usage() {
  cat <<'USAGE'
Usage: tetsu-vm start [options]
  --disk IMG            virtio disk image
  --iso ISO             boot ISO (at least one of --disk / --iso)
  --state DIR           state directory (default <repo>/target/vm/default)
  --mem MB              memory (default 2048)
  --cpus N              vCPUs (default 2)
  --snapshot            discard writes to --disk on exit (VARS and TPM persist)
  --vars TEMPLATE       VARS template (default: distro empty VARS = setup mode)
  --fresh               reset VARS and TPM state first
  --cmdline-extra STR   kernel cmdline addition (systemd-boot)
  --credential N=V      systemd credential (repeatable)
  --serial-console      kernel console and getty on ttyS0
  --gui                 show a display window
  --dry-run             print the QEMU command line and exit
USAGE
}

# Parsed options (globals, set by parse_start_args).
O_DISK="" O_ISO="" O_STATE="$DEFAULT_STATE" O_MEM=2048 O_CPUS=2 O_SNAPSHOT=0
O_VARS="" O_FRESH=0 O_CMDLINE="" O_SERIAL=0 O_GUI=0 O_DRY=0
O_CREDS=()

parse_start_args() {
  while [ $# -gt 0 ]; do
    case "$1" in
      --disk | --iso | --state | --mem | --cpus | --vars | --cmdline-extra | --credential)
        [ $# -ge 2 ] || usage_die "$1 needs a value"
        case "$1" in
          --disk) O_DISK=$2 ;;
          --iso) O_ISO=$2 ;;
          --state) O_STATE=$2 ;;
          --mem) O_MEM=$2 ;;
          --cpus) O_CPUS=$2 ;;
          --vars) O_VARS=$2 ;;
          --cmdline-extra) O_CMDLINE=$2 ;;
          --credential) O_CREDS+=("$2") ;;
        esac
        shift 2
        ;;
      --snapshot) O_SNAPSHOT=1; shift ;;
      --fresh) O_FRESH=1; shift ;;
      --serial-console) O_SERIAL=1; shift ;;
      --gui) O_GUI=1; shift ;;
      --dry-run) O_DRY=1; shift ;;
      -h | --help) start_usage; exit 0 ;;
      *) usage_die "start: unknown option '$1'" ;;
    esac
  done
  [ -n "$O_DISK$O_ISO" ] || usage_die "start: give at least one of --disk / --iso"
  [ -z "$O_DISK" ] || [ -r "$O_DISK" ] || die "disk image '$O_DISK' not found"
  [ -z "$O_ISO" ] || [ -r "$O_ISO" ] || die "ISO '$O_ISO' not found (try tools/vm/fetch-arch-iso)"
  case "$O_MEM$O_CPUS" in *[!0-9]*) usage_die "--mem and --cpus must be integers" ;; esac
  O_STATE=$(readlink -m "$O_STATE")
}

# build_argv: fill the ARGV array with the QEMU command line.
build_argv() {
  local st=$O_STATE code accel cpu cmdline
  code=$(find_firmware "${TETSU_OVMF_CODE:-}" "OVMF secure boot CODE" "${OVMF_CODE_CANDIDATES[@]}")
  if kvm_usable; then accel=kvm cpu=host; else accel=tcg cpu=max; fi

  ARGV=(qemu-system-x86_64
    -machine "q35,smm=on,accel=$accel" -cpu "$cpu" -m "$O_MEM" -smp "$O_CPUS"
    -global driver=cfi.pflash01,property=secure,value=on
    -drive "if=pflash,format=raw,unit=0,readonly=on,file=$(qesc "$code")"
    -drive "if=pflash,format=raw,unit=1,file=$(qesc "$st/vars.fd")"
    -chardev "socket,id=chrtpm,path=$(qesc "$st/swtpm.sock")"
    -tpmdev emulator,id=tpm0,chardev=chrtpm -device tpm-crb,tpmdev=tpm0)
  # snapshot=on on the disk drive only: a global -snapshot would also redirect
  # writes to the VARS pflash. The ISO is read-only already.
  local snap=""
  [ "$O_SNAPSHOT" -eq 0 ] || snap=",snapshot=on"
  [ -z "$O_DISK" ] || ARGV+=(-drive "file=$(qesc "$O_DISK"),if=virtio$snap")
  [ -z "$O_ISO" ] || ARGV+=(-drive "file=$(qesc "$O_ISO"),media=cdrom,readonly=on")

  cmdline=$O_CMDLINE
  if [ "$O_SERIAL" -eq 1 ]; then
    cmdline="console=ttyS0,115200${cmdline:+ $cmdline}"
  fi
  [ -z "$cmdline" ] || ARGV+=(-smbios "type=11,value=io.systemd.boot.kernel-cmdline-extra=$(qesc "$cmdline")")
  [ "$O_SERIAL" -eq 0 ] || ARGV+=(-smbios "type=11,value=io.systemd.credential:getty.ttys.serial=ttyS0")
  local c
  for c in ${O_CREDS[@]+"${O_CREDS[@]}"}; do
    ARGV+=(-smbios "type=11,value=io.systemd.credential:$(qesc "$c")")
  done

  # Keep QEMU's default devices: with -nodefaults the firmware prints nothing on serial.
  [ "$O_GUI" -eq 1 ] || ARGV+=(-display none)
  ARGV+=(
    -chardev "socket,id=ser0,path=$(qesc "$st/serial.sock"),server=on,wait=off,logfile=$(qesc "$st/serial.log")"
    -serial chardev:ser0
    -qmp "unix:$(qesc "$st/qmp.sock"),server=on,wait=off"
    -pidfile "$st/qemu.pid" -daemonize)
}

prepare_state() {
  local st=$O_STATE vars_src
  mkdir -p "$st"
  terminate_stale_swtpm "$st"
  if [ "$O_FRESH" -eq 1 ]; then
    rm -rf "$st/tpm" "$st/vars.fd" "$st/serial.log"
  fi
  mkdir -p "$st/tpm"
  if [ ! -e "$st/vars.fd" ]; then
    vars_src=${O_VARS:-$(find_firmware "${TETSU_OVMF_VARS:-}" "OVMF VARS template" "${OVMF_VARS_CANDIDATES[@]}")}
    [ -r "$vars_src" ] || die "VARS template '$vars_src' not readable"
    cp "$vars_src" "$st/vars.fd"
  fi
  rm -f "$st/swtpm.sock" "$st/qmp.sock" "$st/serial.sock"
  : >>"$st/serial.log"
}

start_swtpm() {
  local st=$O_STATE
  swtpm socket --tpmstate "dir=$st/tpm" --ctrl "type=unixio,path=$st/swtpm.sock" \
    --tpm2 --daemon --pid "file=$st/swtpm.pid" --log "file=$st/swtpm.log" \
    || die "swtpm failed to start (see $st/swtpm.log)"
}

# A swtpm left over from an earlier run must not be orphaned or have its tpm/
# directory removed under it. The pid's command name is checked first.
terminate_stale_swtpm() {
  local pid
  if pid=$(swtpm_pid "$1"); then
    warn "terminating leftover swtpm (pid $pid) of $1"
    term_swtpm "$1"
  fi
}

cmd_start() {
  parse_start_args "$@"
  [ -z "$O_VARS" ] || [ -r "$O_VARS" ] || die "VARS template '$O_VARS' not found"
  build_argv
  if [ "$O_DRY" -eq 1 ]; then
    print_argv
    return 0
  fi
  need_cmd qemu-system-x86_64
  need_cmd swtpm
  if qemu_pid "$O_STATE" >/dev/null; then
    die "a VM is already running in $O_STATE (use 'tetsu-vm stop' or 'power-cut')"
  fi
  prepare_state
  start_swtpm
  if ! "${ARGV[@]}" 2>"$O_STATE/qemu.log"; then
    term_swtpm "$O_STATE"
    die "QEMU failed to start: $(cat "$O_STATE/qemu.log")"
  fi
  qemu_pid "$O_STATE" >/dev/null || { term_swtpm "$O_STATE"; die "QEMU did not leave a live pidfile"; }
  printf 'started: state=%s qemu pid=%s\n' "$O_STATE" "$(qemu_pid "$O_STATE")"
}
