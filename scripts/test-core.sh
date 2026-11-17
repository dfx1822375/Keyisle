#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
build_root="${PASSWORD_MANAGER_BUILD_ROOT:-/mnt/e/CodexBuild/windows-password-manager}"
export CARGO_HOME="$build_root/cargo"
export CARGO_TARGET_DIR="$build_root/target"
export CARGO_BUILD_JOBS=4
cargo test --locked --release --lib
