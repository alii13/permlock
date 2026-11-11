#!/usr/bin/env bash
# Times LavaMoat's policy generation against permlock's on one project, then checks
# that both wrote the same lavamoat/node/policy.json.
# Usage: scripts/bench.sh <project-dir> <entry-file> [runs]
# The project needs @lavamoat/node installed and `permlock` on PATH (or PERMLOCK=...).
set -euo pipefail

dir="$(cd "$1" && pwd)"
entry="$2"
runs="${3:-5}"
permlock="${PERMLOCK:-permlock}"
policy="$dir/lavamoat/node/policy.json"
scratch="$(mktemp -d)"
[ -f "$policy" ] && cp "$policy" "$scratch/original.json"

seconds() {
  /usr/bin/time -p "$@" >/dev/null 2>"$scratch/time" || { cat "$scratch/time" >&2; exit 1; }
  awk '/^real/ { print $2 }' "$scratch/time"
}

cd "$dir"
echo "lavamoat generate:"
for _ in $(seq "$runs"); do echo "  $(seconds ./node_modules/.bin/lavamoat generate "$entry") s"; done
cp "$policy" "$scratch/lavamoat.json"

echo "permlock generate --lavamoat:"
for _ in $(seq "$runs"); do
  echo "  $(seconds "$permlock" generate "$dir" --entry "$entry" --out "$scratch/permlock" --lavamoat) s"
done

if cmp -s "$scratch/lavamoat.json" "$scratch/permlock/lavamoat/node/policy.json"; then
  echo "policies are byte-identical"
else
  echo "policies differ:"
  diff "$scratch/lavamoat.json" "$scratch/permlock/lavamoat/node/policy.json" || true
fi
[ -f "$scratch/original.json" ] && cp "$scratch/original.json" "$policy"
echo "$(uname -sm), node $(node -v)"
