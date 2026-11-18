#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
build_root="${PASSWORD_MANAGER_BUILD_ROOT:-/mnt/e/CodexBuild/windows-password-manager}"
export CARGO_HOME="$build_root/cargo"
export CARGO_TARGET_DIR="${PASSWORD_MANAGER_TARGET_DIR:-$build_root/target}"
export XWIN_CACHE_DIR="$build_root/xwin"
export XWIN_ARCH=x86_64
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}"
export PATH="$build_root/tools:$PATH"
export CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS="-C target-feature=+crt-static --remap-path-prefix=$CARGO_HOME/registry/src/=/cargo/ --remap-path-prefix=$CARGO_TARGET_DIR/=/target/ --remap-path-prefix=$PWD=/keyisle/"
source scripts/prepare-sqlcipher.sh
cargo xwin build --locked --release --target x86_64-pc-windows-msvc
mkdir -p dist
cp "$CARGO_TARGET_DIR/x86_64-pc-windows-msvc/release/windows-password-manager.exe" dist/
