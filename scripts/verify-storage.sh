#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
build_root="${PASSWORD_MANAGER_BUILD_ROOT:-/mnt/e/CodexBuild/windows-password-manager}"
export CARGO_HOME="$build_root/cargo"
export CARGO_TARGET_DIR="$build_root/target"
export XWIN_CACHE_DIR="$build_root/xwin"
export XWIN_ARCH=x86_64
export CARGO_BUILD_JOBS=4
export SQLCIPHER_STATIC=1
export PATH="$build_root/tools:$PATH"
export CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS="-C target-feature=+crt-static"
cargo test --locked --release --lib
cargo build --locked --release --example storage-check
source scripts/prepare-sqlcipher.sh
cargo xwin build --locked --release --target x86_64-pc-windows-msvc --example storage-check --example hello-status
check_linux="$CARGO_TARGET_DIR/release/examples/storage-check"
check_windows="$CARGO_TARGET_DIR/x86_64-pc-windows-msvc/release/examples/storage-check.exe"
fixture_dir=$(mktemp -d "$build_root/storage-validation.XXXXXX")
trap 'rm -rf -- "$fixture_dir"' EXIT
"$check_linux"
"$check_windows"
"$check_linux" --write-fixture "$fixture_dir/linux.db"
"$check_windows" --read-fixture "$(wslpath -w "$fixture_dir/linux.db")"
"$check_windows" --write-fixture "$(wslpath -w "$fixture_dir/windows.db")"
"$check_linux" --read-fixture "$fixture_dir/windows.db"
wc -c "$check_windows"
