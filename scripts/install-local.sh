#!/usr/bin/env bash
# Build and safely install TapPrint on this Mac. Compatible with Bash 3.2.
set -Eeuo pipefail

APP_NAME="TapPrint"

usage() {
  cat <<EOF
Build ${APP_NAME} from this checkout and install it on this Mac.

Usage: scripts/install-local.sh [options]

Options:
  --user          Install to ~/Applications (no admin rights needed)
                  instead of /Applications.
  --dest DIR      Install into DIR instead.
  --no-launch     Don't open the app after installing.
  --force         If ${APP_NAME} won't quit (e.g. a receipt is printing),
                  force it closed. Without this, the script stops and asks you
                  to finish or quit it first.
  -h, --help      Show this help.

Environment:
  CARGO_TARGET_DIR   Honoured for the Rust build output.
  NO_COLOR           Disable coloured output.
EOF
}

# ---- options ---------------------------------------------------------------

DEST_DIR="/Applications"
LAUNCH=1
FORCE=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --user) DEST_DIR="$HOME/Applications" ;;
    --dest)
      [[ $# -ge 2 && -n "$2" ]] || { usage >&2; exit 2; }
      DEST_DIR="$2"
      shift
      ;;
    --dest=*) DEST_DIR="${1#--dest=}" ;;
    --no-launch) LAUNCH=0 ;;
    --force) FORCE=1 ;;
    -h | --help) usage; exit 0 ;;
    *)
      printf 'Unknown option: %s\n\n' "$1" >&2
      usage >&2
      exit 2
      ;;
  esac
  shift
done

# ---- output helpers --------------------------------------------------------

if [[ -t 1 && -z "${NO_COLOR:-}" ]]; then
  BOLD=$'\033[1m' DIM=$'\033[2m' RED=$'\033[31m' GREEN=$'\033[32m' YELLOW=$'\033[33m' RESET=$'\033[0m'
else
  BOLD="" DIM="" RED="" GREEN="" YELLOW="" RESET=""
fi
STEP=0
TOTAL_STEPS=4
step() { STEP=$((STEP + 1)); printf '\n%s[%d/%d] %s%s\n' "$BOLD" "$STEP" "$TOTAL_STEPS" "$1" "$RESET"; }
info() { printf '  %s\n' "$1"; }
ok() { printf '  %s✓%s %s\n' "$GREEN" "$RESET" "$1"; }
warn() { printf '  %s!%s %s\n' "$YELLOW" "$RESET" "$1" >&2; }
fail() {
  printf '\n%serror:%s %s\n' "$RED" "$RESET" "$1" >&2
  [[ -n "${2:-}" ]] && printf '  %s\n' "$2" >&2
  exit 1
}
trap 'printf "\n%serror:%s unexpected failure on line %s (exit %s)\n" "$RED" "$RESET" "$LINENO" "$?" >&2' ERR

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

# Overridable only so the script can be exercised off-macOS in tests.
PLISTBUDDY="${PLISTBUDDY:-/usr/libexec/PlistBuddy}"
SRC_APP="$ROOT_DIR/dist/${APP_NAME}.app"
[[ -n "$DEST_DIR" ]] || fail "Install folder cannot be empty."
if [[ ! -d "$DEST_DIR" && "$DEST_DIR" == "$HOME/Applications" ]]; then
  mkdir -p "$DEST_DIR"
fi
[[ -d "$DEST_DIR" ]] || fail "Install folder does not exist: $DEST_DIR"
DEST_DIR="$(cd "$DEST_DIR" && pwd -P)"
DST_APP="${DEST_DIR}/${APP_NAME}.app"
START_TS=$(date +%s)

# ---- 1. preflight ----------------------------------------------------------

step "Checking your setup"

[[ "$(uname -s)" == "Darwin" ]] || fail "This script installs a macOS app and must run on a Mac."

need() {
  command -v "$1" >/dev/null 2>&1 || fail "Missing required tool: $1" "$2"
}
need git "Install the Xcode Command Line Tools: xcode-select --install"
need codesign "Install the Xcode Command Line Tools: xcode-select --install"
need ditto "ditto ships with macOS; is your PATH unusual?"
[[ -x "$PLISTBUDDY" ]] || fail "PlistBuddy not found at $PLISTBUDDY" "It ships with macOS at /usr/libexec/PlistBuddy."
if command -v xcode-select >/dev/null 2>&1 && ! xcode-select -p >/dev/null 2>&1; then
  fail "Xcode Command Line Tools are not installed." "Run: xcode-select --install, then re-run this script."
fi
need cargo "Install Rust from https://rustup.rs, then open a new terminal."
need rustc "Install Rust from https://rustup.rs, then open a new terminal."

need swift "Install the Xcode Command Line Tools: xcode-select --install"
need iconutil "iconutil ships with macOS."
need osascript "osascript ships with macOS."
ok "Rust and macOS bundle tools found"

# Destination must exist and be writable before we spend minutes building.
if [[ ! -d "$DEST_DIR" ]]; then
  if [[ "$DEST_DIR" == "$HOME/Applications" ]]; then
    mkdir -p "$DEST_DIR"
  else
    fail "Install folder does not exist: $DEST_DIR"
  fi
fi
[[ -w "$DEST_DIR" ]] || fail "Can't write to $DEST_DIR." "Re-run with --user to install into ~/Applications instead."
ok "Installing to $DST_APP"

# Stamp the source commit, with -dirty for uncommitted changes.
SHA="$(git rev-parse --short HEAD 2>/dev/null || echo unknown)"
if [[ -n "$(git status --porcelain 2>/dev/null || true)" ]]; then
  SHA="${SHA}-dirty"
  warn "You have uncommitted changes — the build is stamped ${SHA}."
fi

# ---- 2. build --------------------------------------------------------------

step "Building ${APP_NAME} (${SHA})"
"$ROOT_DIR/scripts/macos-app.sh" || fail "Build failed; installed app was not touched."
[[ -d "$SRC_APP" ]] || fail "App bundle missing: $SRC_APP"

# Stamp the native About panel. CFBundleVersion renders as "(…)" beside the
# short version; set it to the source commit.
"$PLISTBUDDY" -c "Set :CFBundleVersion $SHA" "$SRC_APP/Contents/Info.plist"

# Ad-hoc re-sign: the Info.plist edit invalidates the bundle signature, and
# Gatekeeper needs a valid (even ad-hoc) one to launch a local build.
codesign --force --sign - "$SRC_APP"
codesign --verify "$SRC_APP" || fail "Code signature check failed for the new build."
ok "Built and signed"

# ---- 3. install ------------------------------------------------------------

step "Installing"

# Match only the installed copy's binary, never a dev build or another app.
APP_PROCESS="${DST_APP}/Contents/MacOS/tapprint"
# Compare executable paths literally; pgrep -f treats paths as regexes.
is_running() { pids="$(pgrep -x tapprint || true)"; for pid in $pids; do
  [[ "$(ps -p "$pid" -o comm=)" == "$APP_PROCESS" ]] && return 0
 done; return 1; }
stop_running() { for pid in $(pgrep -x tapprint || true); do
  [[ "$(ps -p "$pid" -o comm=)" != "$APP_PROCESS" ]] || kill "${1:--TERM}" "$pid"
 done; }

if is_running; then
  info "Quitting the running ${APP_NAME}…"
  osascript - "$DST_APP" <<'APPLESCRIPT' >/dev/null 2>&1 || true
on run argv
  tell application (item 1 of argv) to quit
end run
APPLESCRIPT
  # Give the app time to close cleanly.
  for _ in $(seq 1 20); do
    is_running || break
    sleep 0.5
  done
  if is_running; then
    if [[ "$FORCE" -eq 1 ]]; then
      warn "${APP_NAME} didn't quit — forcing it closed (--force)."
      stop_running -TERM 2>/dev/null || true
      sleep 1
      is_running && stop_running -KILL 2>/dev/null || true
    else
      fail "${APP_NAME} is still open — it may be asking to confirm a print job." \
        "Finish or quit it yourself, then re-run. (Or pass --force to close it anyway.)"
    fi
  fi
  is_running && fail "TapPrint is still running; install cancelled."
  ok "Closed the old copy"
fi

# Atomic swap: copy the new build next to the destination, move the old copy
# aside, move the new one into place, then delete the old one. If any step
# fails, the previous install is restored — you're never left without the app.
STAGE="${DEST_DIR}/.${APP_NAME}.app.installing.$$"
BACKUP="${DEST_DIR}/.${APP_NAME}.app.previous.$$"
COMMITTED=0
cleanup() {
  rm -rf "$STAGE"
  if [[ "$COMMITTED" -eq 0 && -d "$BACKUP" ]]; then
    rm -rf "$DST_APP"
    mv "$BACKUP" "$DST_APP" && warn "Restored your previous ${APP_NAME}."
  fi
}
trap cleanup EXIT

[[ "$DST_APP" != "$SRC_APP" ]] || fail "Destination must differ from the build folder."
[[ ! -e "$DST_APP" || -d "$DST_APP" ]] || fail "Destination exists and is not an app folder."
[[ ! -L "$DST_APP" ]] || fail "Refusing to replace a symlink: $DST_APP"
ditto "$SRC_APP" "$STAGE"
codesign --verify "$STAGE"
if [[ -d "$DST_APP" ]]; then
  mv "$DST_APP" "$BACKUP"
fi
mv "$STAGE" "$DST_APP"

# Confirm the installed bundle is the build we just stamped: its CFBundleVersion
# must equal this commit's sha, else an old copy survived the swap.
INSTALLED_SHA="$("$PLISTBUDDY" -c 'Print :CFBundleVersion' "$DST_APP/Contents/Info.plist")"
if [[ "$INSTALLED_SHA" != "$SHA" ]]; then
  rm -rf "$DST_APP"
  fail "Installed version ($INSTALLED_SHA) doesn't match this build ($SHA)."
fi
codesign --verify "$DST_APP"
COMMITTED=1
rm -rf "$BACKUP"
VER="$("$PLISTBUDDY" -c 'Print :CFBundleShortVersionString' "$DST_APP/Contents/Info.plist")"
ok "Installed ${APP_NAME} ${VER} (${SHA})"

# ---- 4. launch -------------------------------------------------------------

step "Finishing up"
if [[ "$LAUNCH" -eq 1 ]]; then
  open "$DST_APP"
  ok "Opened ${APP_NAME}"
else
  info "Not launching (--no-launch). Open it from ${DEST_DIR}."
fi

ELAPSED=$(($(date +%s) - START_TS))
printf '\n%s%s %s (%s) is installed%s %s— took %dm %02ds%s\n' \
  "$GREEN" "$BOLD" "$APP_NAME" "$SHA" "$RESET" "$DIM" $((ELAPSED / 60)) $((ELAPSED % 60)) "$RESET"
