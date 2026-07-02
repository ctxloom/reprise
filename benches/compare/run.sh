#!/usr/bin/env bash
# §7.3 baseline comparison driver (spec §7.3). Runs jscpd (and PMD/CPD if the
# `pmd` binary is on PATH — needs java) on a repo, runs reprise on the same
# repo, and diffs the findings with compare.py.
#
#   run.sh <repo-path> <lang>       # lang = jscpd format: rust | python | ...
#
# Availability: jscpd is fetched via `npx --yes jscpd` (needs node/npx). PMD CPD
# is used only if a `pmd` executable is found. If NEITHER is available the
# script prints an honest note and exits 3, leaving this harness runnable later.
set -euo pipefail

REPO="${1:?usage: run.sh <repo-path> <lang>}"
LANG_FMT="${2:?usage: run.sh <repo-path> <lang>}"
HERE="$(cd "$(dirname "$0")" && pwd)"
REPRISE_BIN="${REPRISE_BIN:-$HERE/../../target/release/reprise}"
OUT="${OUT:-$(mktemp -d)}"
mkdir -p "$OUT"
echo "output dir: $OUT"

have_node=0; have_java=0; ran_any=0
command -v npx >/dev/null 2>&1 && have_node=1
command -v java >/dev/null 2>&1 && have_java=1

# --- reprise (jscpd-shaped + full JSON) ---
"$REPRISE_BIN" scan "$REPO" --format json  > "$OUT/reprise.json"
"$REPRISE_BIN" scan "$REPO" --format jscpd > "$OUT/reprise-jscpd.json"
echo "reprise: wrote reprise.json + reprise-jscpd.json"

# --- jscpd ---
if [ "$have_node" = 1 ]; then
  npx --yes jscpd -k 30 -r json -o "$OUT" -s "$REPO" >/dev/null 2>&1 || true
  if [ -f "$OUT/jscpd-report.json" ]; then
    ran_any=1
    echo "== compare vs jscpd =="
    python3 "$HERE/compare.py" --reprise "$OUT/reprise.json" \
      --jscpd "$OUT/jscpd-report.json" --lang "$LANG_FMT" --root "$REPO" \
      --json "$OUT/compare-jscpd.json"
  fi
else
  echo "npx/node not available — skipping jscpd"
fi

# --- PMD CPD (optional; needs java + a pmd install) ---
if [ "$have_java" = 1 ] && command -v pmd >/dev/null 2>&1; then
  pmd cpd --minimum-tokens 30 --dir "$REPO" --language "$LANG_FMT" --format xml \
    > "$OUT/pmd-cpd.xml" 2>/dev/null || true
  echo "PMD CPD xml at $OUT/pmd-cpd.xml (compare manually or extend compare.py)"
elif [ "$have_java" = 1 ]; then
  echo "java present but no 'pmd' binary on PATH — PMD CPD skipped"
fi

if [ "$ran_any" = 0 ]; then
  echo "NEITHER jscpd nor PMD produced output — comparison unavailable on this host."
  echo "Harness is runnable later; install node (jscpd) or PMD (java) and re-run."
  exit 3
fi
