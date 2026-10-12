#!/bin/sh
# WASM-SPIKE: readable view of the mirrored browser log.  usage: logs.sh [tail-lines]
grep -vE '^at |^Stack|^Error$|^$|menu catalog|render frame diag|sound bank|HOME or XDG' /tmp/iw4_browser.log 2>/dev/null \
  | sed -E 's/^\[error\] //' | cut -c1-220 | tail -"${1:-40}"
echo "-- requests: $(wc -l < /tmp/iw4_req.log 2>/dev/null) | panics: $(grep -c panicked /tmp/iw4_browser.log 2>/dev/null)"
