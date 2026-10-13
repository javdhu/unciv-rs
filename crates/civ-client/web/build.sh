#!/bin/sh
# The threaded wasm client, one build per graphics backend (docs/CLIENT-BRIEF.md, D9; docs/VISUAL-SPEC.md, V5).
# Usage: [PROFILE=web-dev] crates/civ-client/web/build.sh [webgpu] [webgl2]   (both when none is named)
# PROFILE defaults to web (thin LTO, measured and shipped); web-dev skips LTO for faster iteration.
# Needs the pinned nightly with rust-src and the wasm32 target, and wasm-bindgen-cli matching Cargo.lock.
set -eu
root=$(cd "$(dirname "$0")/../../.." && pwd)
web="$root/crates/civ-client/web"
toolchain=nightly-2026-10-08
profile=${PROFILE:-web}
# Every crate is compiled with atomics; only the final link sees the shared-memory arguments. Rust no longer
# turns shared memory on by itself with +atomics, and wasm-bindgen's thread support needs the memory imported
# and the TLS symbols exported. The memory may grow to 4 GiB; the main thread gets an 8 MiB stack like the
# engine thread's (web/worker.js).
# The nightly also gets a parallel front end and shared generics, which cut compile time.
features='-C target-feature=+atomics,+bulk-memory,+mutable-globals -Z threads=8 -Z share-generics=y'
link='-C link-arg=--shared-memory -C link-arg=--import-memory -C link-arg=--max-memory=4294967296
  -C link-arg=--export=__wasm_init_tls -C link-arg=--export=__tls_size -C link-arg=--export=__tls_align
  -C link-arg=--export=__tls_base -C link-arg=-zstack-size=8388608'
for flavor in ${*:-webgpu webgl2}; do
  # shellcheck disable=SC2086 # $link is a list of arguments
  RUSTFLAGS="$features" cargo "+$toolchain" rustc --manifest-path "$root/Cargo.toml" -p civ-client --bin civ-client \
    --profile "$profile" --target wasm32-unknown-unknown -Z build-std=std,panic_abort --features "$flavor" -- $link
  rm -rf "$web/pkg/$flavor"
  wasm-bindgen --target web --no-typescript --out-dir "$web/pkg/$flavor" --out-name civ_client \
    "$root/target/wasm32-unknown-unknown/$profile/civ-client.wasm"
  cp "$web/worker.js" "$web/pkg/$flavor/worker.js"
done
