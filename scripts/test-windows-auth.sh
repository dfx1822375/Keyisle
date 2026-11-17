#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
build_root="${PASSWORD_MANAGER_BUILD_ROOT:-/mnt/e/CodexBuild/windows-password-manager}"
export CARGO_HOME="$build_root/cargo"
export CARGO_TARGET_DIR="$build_root/target"
export XWIN_CACHE_DIR="$build_root/xwin"
export XWIN_ARCH=x86_64
export CARGO_BUILD_JOBS=4
export PATH="$build_root/tools:$PATH"
export CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS="-C target-feature=+crt-static"
source scripts/prepare-sqlcipher.sh
case "${1:-}" in
  --lib) test_target=(--lib); label=core ;;
  '') test_target=(--bin windows-password-manager); label=auth ;;
  *) echo 'Usage: test-windows-auth.sh [--lib]' >&2; exit 2 ;;
esac
test_output="$build_root/diagnostics/native-tests"
mkdir -p "$test_output"
cargo xwin test --locked --release --target x86_64-pc-windows-msvc "${test_target[@]}" --no-run --message-format=json > "$test_output/windows-$label-test-build.jsonl"
python3 - "$test_output" "$label" <<'PYTHON'
import json, pathlib, sys
root=pathlib.Path(sys.argv[1])
label=sys.argv[2]
artifacts=[json.loads(line) for line in (root/f'windows-{label}-test-build.jsonl').read_text().splitlines() if line.startswith('{')]
executables=[item['executable'] for item in artifacts if item.get('reason')=='compiler-artifact' and item.get('profile',{}).get('test') and item.get('executable')]
assert len(executables)==1, executables
(root/f'windows-{label}-test-path.txt').write_text(executables[0])
script="param([string]$TestExe,[string]$BuildRoot)\n$process=Start-Process -FilePath $TestExe -ArgumentList '--nocapture' -Wait -PassThru -RedirectStandardOutput (Join-Path $BuildRoot 'windows-LABEL-test.stdout.log') -RedirectStandardError (Join-Path $BuildRoot 'windows-LABEL-test.stderr.log')\nGet-Content (Join-Path $BuildRoot 'windows-LABEL-test.stdout.log')\nGet-Content (Join-Path $BuildRoot 'windows-LABEL-test.stderr.log')\nexit $process.ExitCode\n"
(root/f'run-windows-{label}-tests.ps1').write_text(script.replace('LABEL', label),encoding='utf-8-sig')
PYTHON
artifact="$(cat "$test_output/windows-$label-test-path.txt")"
/mnt/c/Windows/System32/WindowsPowerShell/v1.0/powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$(wslpath -w "$test_output/run-windows-$label-tests.ps1")" -TestExe "$(wslpath -w "$artifact")" -BuildRoot "$(wslpath -w "$test_output")"
