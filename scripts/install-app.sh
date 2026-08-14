#!/bin/sh
# Installs the release bundle into /Applications. Builds nothing: run
# `npm run tauri build` first, or `npm run release` to do both.
#
# Idempotent, and indifferent to where it is called from — it resolves its own
# paths rather than trusting the working directory.
#
# Two things this does that a plain `cp -R` does not, and both are why it starts
# by uninstalling.
#
# It quits Press first. Replacing the bundle under a running process leaves that
# process with a code signature that no longer resolves, and macOS then starts
# refusing it access to protected folders — the app keeps running but stops
# working, in a way that looks like anything but an update.
#
# It removes the old bundle rather than copying over it. `cp -R` onto an
# existing directory merges into it, so a file that disappears between versions
# is left behind forever.
#
# Both of those are what `uninstall-app.sh` already does, so it does them,
# rather than this script keeping a second copy of the same care.
set -eu

root=$(
	CDPATH= cd -- "$(dirname -- "$0")/.." && pwd
)
BUNDLE="$root/src-tauri/target/release/bundle/macos/Press.app"
TARGET="/Applications/Press.app"

if [ ! -d "$BUNDLE" ]; then
	echo "No release bundle at $BUNDLE" >&2
	echo "Run 'npm run tauri build' first, or 'npm run release' to do both." >&2
	exit 1
fi

sh "$root/scripts/uninstall-app.sh" --quiet

# ditto rather than cp: it is the macOS-native copy and keeps extended
# attributes and the signature intact.
ditto "$BUNDLE" "$TARGET"

if [ ! -x "$TARGET/Contents/MacOS/press" ]; then
	echo "Copied $TARGET but its binary is missing or not executable." >&2
	exit 1
fi

echo "Installed $TARGET"
codesign -dv "$TARGET" 2>&1 | grep -E "^Identifier" || true
du -sh "$TARGET" | awk '{print "  " $1}'
