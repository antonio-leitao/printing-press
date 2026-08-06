#!/bin/sh
# Installs the release bundle into /Applications.
#
# Two things this does that a plain `cp -R` does not.
#
# It quits Press first. Replacing the bundle under a running process leaves that
# process with a code signature that no longer resolves, and macOS then starts
# refusing it access to protected folders — the app keeps running but stops
# working, in a way that looks like anything but an update.
#
# It removes the old bundle before copying. `cp -R` onto an existing directory
# merges into it, so a file that disappears between versions is left behind
# forever.
set -eu

BUNDLE="src-tauri/target/release/bundle/macos/Press.app"
TARGET="/Applications/Press.app"

if [ ! -d "$BUNDLE" ]; then
	echo "No release bundle at $BUNDLE — run 'npm run tauri build' first." >&2
	exit 1
fi

if pgrep -x press >/dev/null 2>&1; then
	echo "Quitting the running Press…"
	osascript -e 'quit app "Press"' >/dev/null 2>&1 || true
	# Give it a moment to exit cleanly before insisting.
	for _ in 1 2 3 4 5 6 7 8 9 10; do
		pgrep -x press >/dev/null 2>&1 || break
		sleep 0.3
	done
	pkill -x press >/dev/null 2>&1 || true
fi

rm -rf "$TARGET"
# ditto rather than cp: it is the macOS-native copy and keeps extended
# attributes and the signature intact.
ditto "$BUNDLE" "$TARGET"

echo "Installed $TARGET"
codesign -dv "$TARGET" 2>&1 | grep -E "^Identifier" || true
du -sh "$TARGET" | awk '{print "  " $1}'
