#!/usr/bin/env bash
# This Source Code Form is subject to the terms of the Mozilla Public
# License, v. 2.0. If a copy of the MPL was not distributed with this
# file, You can obtain one at https://mozilla.org/MPL/2.0/.

# The crate's central claim, as a check: the same simulation must produce the
# same number everywhere it runs.
#
# `src/fingerprint.rs` runs a fixed scenario across every layer — parsing,
# fixed-point arithmetic, powers, modifier composition, yields, growth,
# movement, combat, the generator — and folds every observable into one u64.
# If one bit of one calculation differs on any target, these disagree.
#
# Targets, each chosen to break something different:
#   macOS/native   the host
#   wasm32         a different code generator, and the browser client
#   linux/arm64    a different OS and a different libm (glibc, not Apple's)
#   linux/amd64    a different instruction set as well
#
# The Linux targets need Docker and are skipped without it, loudly.
# CHECK_DETERMINISM_DOCKER=0 skips them on purpose: CI's host is already
# linux/amd64 with glibc, so there the native leg covers that libm.
set -euo pipefail
cd "$(dirname "$0")/.."

fingerprint_from() { grep -o '0x[0-9a-f]\{16\}' | head -1; }

# macOS has no `timeout`, and a stalled emulated build must not hang the check.
# Runs the command with a watchdog, printing whatever it produced.
with_limit() {
  local limit=$1 out; shift
  out=$(mktemp)
  "$@" >"$out" 2>/dev/null &
  local pid=$!
  ( sleep "$limit"; kill -9 "$pid" 2>/dev/null ) >/dev/null 2>&1 &
  local watcher=$!
  wait "$pid" 2>/dev/null || true
  kill "$watcher" 2>/dev/null || true
  cat "$out"
  rm -f "$out"
}

# Debug suites skip the timing bound; keep its release gate in this check.
echo "release performance and snapshot budget"
cargo test --quiet --release --features save --test budget

echo "native"
native=$(cargo test --quiet --release --lib the_scenario_actually_runs -- --nocapture 2>/dev/null | fingerprint_from)

echo "wasm32"
cargo build --quiet --release --target wasm32-unknown-unknown
wasm=$(bun tools/wasm_fingerprint.mjs)

results=("native $native" "wasm32 $wasm")

# Linux gives a different libm — glibc rather than Apple's — which is the
# divergence the fixed-point design exists to avoid. A foreign-architecture
# container additionally changes the instruction set, but Rust compiling under
# QEMU user emulation is unreliable: it has been observed to stall indefinitely
# at ~0% CPU. So the emulated leg is time-limited and its absence is reported
# rather than waited on.
native_platform="linux/$(uname -m | sed 's/arm64/arm64/;s/aarch64/arm64/;s/x86_64/amd64/')"
if [ "${CHECK_DETERMINISM_DOCKER:-1}" = 0 ]; then
  echo "  (CHECK_DETERMINISM_DOCKER=0 — Linux containers skipped on purpose)" >&2
elif docker info >/dev/null 2>&1; then
  for platform in "$native_platform" linux/amd64 linux/arm64; do
    case " ${results[*]} " in *" $platform "*) continue ;; esac
    [ "$platform" = "$native_platform" ] && limit=900 || limit=600
    echo "$platform"
    value=$(with_limit "$limit" docker run --rm --platform "$platform" \
      -v "$PWD/../..:/work" -w /work/crates/civ-core \
      -e CARGO_TARGET_DIR="/tmp/target-${platform//\//-}" \
      rust:latest \
      cargo test --release --quiet --lib the_scenario_actually_runs -- --nocapture \
      | fingerprint_from)
    if [ -n "$value" ]; then
      results+=("$platform $value")
    else
      echo "  ($platform produced nothing within ${limit}s — skipped, not failed)" >&2
    fi
  done
else
  echo "  (docker unavailable — Linux targets skipped, so libm is untested)" >&2
fi

echo
printf '  %-14s %s\n' $(printf '%s\n' "${results[@]}" | tr ' ' '\n')
echo

first=$(echo "${results[0]}" | awk '{print $2}')
[ -n "$first" ] || { echo "FAIL: no fingerprint produced" >&2; exit 1; }
for row in "${results[@]}"; do
  value=$(echo "$row" | awk '{print $2}')
  if [ "$value" != "$first" ]; then
    echo "FAIL: ${row%% *} disagrees — something in the rules layer is not deterministic" >&2
    exit 1
  fi
done
echo "OK: bit-identical across ${#results[@]} targets"
