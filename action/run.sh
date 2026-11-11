#!/usr/bin/env bash
# Runs permlock for the GitHub Action and copies its report into the job summary.
# Inputs arrive as environment variables, never interpolated into the script.
set -uo pipefail

mode="${PERMLOCK_MODE:-check}"
case "$mode" in
  check | verify) ;;
  *)
    echo "permlock action: mode must be check or verify, got '$mode'" >&2
    exit 2
    ;;
esac

dir="${PERMLOCK_DIR:-.}"
report="$(mktemp)"

if [ -n "${PERMLOCK_BIN:-}" ]; then
  "$PERMLOCK_BIN" "$mode" "$dir" >"$report"
else
  npx --yes "permlock@${PERMLOCK_VERSION:?set the version input}" "$mode" "$dir" >"$report"
fi
status=$?

cat "$report"
if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
  {
    echo "### permlock $mode"
    echo
    echo '```'
    cat "$report"
    echo '```'
  } >>"$GITHUB_STEP_SUMMARY"
fi
exit "$status"
