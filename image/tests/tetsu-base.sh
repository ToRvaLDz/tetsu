#!/usr/bin/env bash
# Pin the shape of the base set: it lives in the depends of the tetsu-base
# meta-package, and the payload installs only that package.
# No root, no network.
# No `-e`: a failed check is counted, not fatal.
set -uo pipefail

REPO=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
PKGBUILD="$REPO/image/tetsu-base/PKGBUILD"
CONF="$REPO/image/mkosi.profiles/payload/mkosi.conf"
CRITICAL=(base linux systemd pacman sudo btrfs-progs cryptsetup archlinux-keyring)

fails=0
pass() { printf 'ok   %s\n' "$1"; }
fail() { printf 'FAIL %s\n' "$1"; fails=$((fails + 1)); }
expect() { # DESCRIPTION CONDITION-EXIT-STATUS
  if [ "$2" -eq 0 ]; then pass "$1"; else fail "$1"; fi
}

# The PKGBUILD is sourced in a subshell; it prints the fields as lines.
if [ -f "$PKGBUILD" ]; then
  fields=$(
    # shellcheck disable=SC1090
    source "$PKGBUILD"
    printf 'pkgname=%s\n' "${pkgname[*]:-}"
    printf 'arch=%s\n' "${arch[*]:-}"
    for d in "${depends[@]:-}"; do [ -n "$d" ] && printf 'dep=%s\n' "$d"; done
  ) || fields=
else
  fields=
fi

name=$(sed -n 's/^pkgname=//p' <<<"$fields")
arch=$(sed -n 's/^arch=//p' <<<"$fields")
mapfile -t deps < <(sed -n 's/^dep=//p' <<<"$fields")

[ "$name" = tetsu-base ]; expect "pkgname is tetsu-base" $?
[ "$arch" = any ]; expect "arch is (any)" $?
[ "${#deps[@]}" -gt 0 ]; expect "depends is not empty" $?
dups=$(printf '%s\n' "${deps[@]}" | sort | uniq -d)
[ -z "$dups" ] && [ "${#deps[@]}" -gt 0 ]; expect "depends has no duplicates" $?
for p in "${CRITICAL[@]}"; do
  printf '%s\n' "${deps[@]}" | grep -qx -- "$p"; expect "depends has $p" $?
done

# Packages= of the payload: the indented continuation lines after the key.
pkgs=$(awk '
  /^Packages=/ { on = 1; sub(/^Packages=/, ""); if ($0 ~ /[^[:space:]]/) print; next }
  on && /^[[:space:]]+[^[:space:]#]/ { print; next }
  on && /^[[:space:]]*#/ { next }
  on { on = 0 }
' "$CONF" | tr -s '[:space:]' '\n' | sed '/^$/d')
[ "$pkgs" = tetsu-base ]; expect "payload Packages= is exactly tetsu-base" $?

if [ "$fails" -ne 0 ]; then
  printf '%d check(s) failed\n' "$fails" >&2
  exit 1
fi
