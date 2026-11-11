#!/usr/bin/env bash
# Builds one permlock-<os>-<cpu> npm package around a prebuilt binary.
# Usage: npm-platform.sh <os-cpu> <version> <binary> <out-dir>
set -euo pipefail

target="$1" version="$2" binary="$3" out="$4"
os="${target%-*}" cpu="${target#*-}"
exe=permlock
[ "$os" = win32 ] && exe=permlock.exe

mkdir -p "$out/bin"
cp "$binary" "$out/bin/$exe"
chmod +x "$out/bin/$exe"
cat >"$out/package.json" <<EOF
{
  "name": "permlock-$target",
  "version": "$version",
  "description": "The permlock binary for $os $cpu. Install the permlock package instead.",
  "repository": {
    "type": "git",
    "url": "git+https://github.com/alii13/permlock.git"
  },
  "license": "MIT OR Apache-2.0",
  "os": ["$os"],
  "cpu": ["$cpu"],
  "files": ["bin"],
  "capabilities": []
}
EOF
echo "built permlock-$target@$version in $out"
