#!/usr/bin/env bash
# Source after setting the common cargo-xwin environment.
native_messages=$(cargo xwin build --locked --release --target x86_64-pc-windows-msvc -p password-manager-sqlcipher --message-format=json)
SQLCIPHER_LIB_DIR=$(printf '%s' "$native_messages" | python3 -c '
import json, sys
paths = [item["out_dir"] for line in sys.stdin if (item := json.loads(line)).get("reason") == "build-script-executed" and "password-manager-sqlcipher" in item["package_id"]]
if not paths: raise SystemExit("SQLCipher build output not found")
print(paths[-1])
')
export SQLCIPHER_LIB_DIR
export SQLCIPHER_INCLUDE_DIR="$SQLCIPHER_LIB_DIR"
export SQLCIPHER_STATIC=1
unset native_messages
