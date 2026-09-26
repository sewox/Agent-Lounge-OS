#!/usr/bin/env bash
# Mac live-test helper (S2): move+maximize "Agent Lounge OS" onto each display,
# navigate routes (via Accessibility / deep-link proposal), screencapture -l <windowid>.
# Run on Sercan's Mac with three displays attached. Requires: osascript, screencapture, Python3.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
OUT_DIR="${QA_MAC_OUT:-$ROOT/docs/qa/mac-live-$(date +%Y-%m-%d)}"
APP_NAME="${QA_APP_NAME:-Agent Lounge OS}"
ROUTES=(dashboard stream vault health fleet telemetry quotas settings onboarding)

mkdir -p "$OUT_DIR"

echo "== QA Mac live capture =="
echo "App: $APP_NAME"
echo "Out: $OUT_DIR"

# Enumerate displays via NSScreen (Python + PyObjC if available; else System Events bounds).
python3 - <<'PY' > "$OUT_DIR/displays.json" || true
import json, sys
try:
    import AppKit  # type: ignore
    screens = []
    for i, s in enumerate(AppKit.NSScreen.screens()):
        f = s.frame()
        screens.append({
            "index": i,
            "width": int(f.size.width),
            "height": int(f.size.height),
            "x": int(f.origin.x),
            "y": int(f.origin.y),
            "backingScaleFactor": float(s.backingScaleFactor()),
        })
    print(json.dumps({"screens": screens}, indent=2))
except Exception as e:
    print(json.dumps({"error": str(e), "screens": []}))
    sys.exit(0)
PY

# Fallback: System Events desktop bounds
osascript <<'APPLESCRIPT' > "$OUT_DIR/displays-system-events.txt" || true
tell application "System Events"
  set out to {}
  repeat with d in desktops
    set end of out to (id of d as string) & " " & (name of d as string)
  end repeat
  return out
end tell
APPLESCRIPT

move_maximize_to_display() {
  local display_index="$1"
  osascript <<APPLESCRIPT
tell application "System Events"
  tell process "$APP_NAME"
    set frontmost to true
    -- Best-effort: move main window; maximize via zoom button.
    try
      set win to window 1
      -- Display positioning depends on arrangement; caller may tweak.
      perform action "AXRaise" of win
      try
        click (button 2 of win) -- zoom / maximize often button 2 on macOS
      end try
    end try
  end tell
end tell
-- Hint for multi-display: use Mission Control / drag if AX fails.
display notification "Moved $APP_NAME toward display $display_index" with title "QA Mac"
APPLESCRIPT
}

window_id() {
  # CGWindowList via Swift/Python is ideal; screencapture -l needs window id.
  # Parse from Swift-less fallback using Python Quartz if available.
  python3 - <<'PY'
import json, sys
try:
    import Quartz  # type: ignore
    app = "Agent Lounge OS"
    opts = Quartz.kCGWindowListOptionOnScreenOnly | Quartz.kCGWindowListExcludeDesktopElements
    wins = Quartz.CGWindowListCopyWindowInfo(opts, Quartz.kCGNullWindowID)
    for w in wins:
        name = w.get("kCGWindowOwnerName") or ""
        title = w.get("kCGWindowName") or ""
        if app in name or app in title:
            print(int(w["kCGWindowNumber"]))
            sys.exit(0)
    print("", end="")
except Exception:
    print("", end="")
PY
}

# Proposed deep-link / route mechanism (NOT implemented in product this PR):
# Option A: custom URL scheme agent-lounge://route/vault
# Option B: query ?qa-route=/vault when running a QA build flag
# Option C: Accessibility click sidebar links (used below)
navigate_route() {
  local route="$1"
  osascript <<APPLESCRIPT
tell application "System Events"
  tell process "$APP_NAME"
    set frontmost to true
    -- Click sidebar link by label heuristics
    try
      click (first UI element of window 1 whose name contains "$route" or value contains "$route")
    end try
  end tell
end tell
APPLESCRIPT
}

DISPLAY_COUNT=$(python3 -c 'import json; print(len(json.load(open("'"$OUT_DIR"'/displays.json")).get("screens",[])))' 2>/dev/null || echo 3)
if [[ "$DISPLAY_COUNT" -lt 1 ]]; then DISPLAY_COUNT=3; fi

for ((d=0; d<DISPLAY_COUNT && d<3; d++)); do
  echo "-- Display $d"
  move_maximize_to_display "$d" || true
  sleep 1
  WID="$(window_id || true)"
  for route in "${ROUTES[@]}"; do
    echo "   route=$route windowId=${WID:-unknown}"
    navigate_route "$route" || true
    sleep 0.8
    if [[ -n "${WID:-}" ]]; then
      screencapture -l "$WID" "$OUT_DIR/display${d}__${route}.png" || \
        screencapture -x "$OUT_DIR/display${d}__${route}.png" || true
    else
      screencapture -x "$OUT_DIR/display${d}__${route}.png" || true
    fi
  done
done

cp "$(dirname "$0")/QA_Report.md" "$OUT_DIR/QA_Report.md"
echo "Done. Fill checklist: $OUT_DIR/QA_Report.md"
echo
echo "Deep-link proposal (do NOT implement in PR-0 product code):"
echo "  Prefer a QA-only query ?qa-nav=/vault gated by env LOUNGE_QA=1, or"
echo "  a custom URL scheme agent-lounge://nav/vault for S2 automation."
echo "  Until then, this script clicks sidebar labels via Accessibility."
