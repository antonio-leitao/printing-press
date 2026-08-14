#!/bin/sh
# Removes Press from /Applications.
#
# What this does not remove, unless asked with --purge: everything Press has
# stored. Snapshots live in Press's own object store — not in your project
# folders, not in git — so `~/Library/Application Support/com.antonio.press` is
# the only copy of every version you ever kept. Removing the app is undone by
# building it again; removing that is not undone at all. So the app goes and the
# history stays, and you have to say `--purge` to mean otherwise.
#
# Idempotent, and indifferent to where it is called from.
set -eu

TARGET="/Applications/Press.app"
DATA="$HOME/Library/Application Support/com.antonio.press"
CACHE="$HOME/Library/Caches/com.antonio.press"
# Left behind by a Press that was killed rather than quit. Harmless — the next
# start clears it — but an uninstall should not leave litter either.
SOCKET="/tmp/com.antonio.press_si.sock"

purge=no
quiet=no
for argument in "$@"; do
	case "$argument" in
	--purge) purge=yes ;;
	--quiet) quiet=yes ;;
	*)
		echo "usage: $0 [--purge] [--quiet]" >&2
		exit 2
		;;
	esac
done

say() {
	if [ "$quiet" = no ]; then
		echo "$@"
	fi
}

# Both the installed app and a development build are called `press`, and both
# should stop: only one Press runs at a time, and the one holding the database
# is whichever started first.
if pgrep -x press >/dev/null 2>&1; then
	say "Quitting the running Press…"
	osascript -e 'quit app "Press"' >/dev/null 2>&1 || true
	# Give it a moment to exit cleanly before insisting.
	for _ in 1 2 3 4 5 6 7 8 9 10; do
		pgrep -x press >/dev/null 2>&1 || break
		sleep 0.3
	done
	pkill -x press >/dev/null 2>&1 || true
fi

removed=no
if [ -d "$TARGET" ]; then
	rm -rf "$TARGET"
	removed=yes
fi
rm -f "$SOCKET"

if [ "$purge" = yes ]; then
	rm -rf "$DATA" "$CACHE"
	say "Removed $TARGET, its history and its caches."
	exit 0
fi

if [ "$removed" = yes ]; then
	say "Removed $TARGET"
else
	say "Nothing to remove at $TARGET"
fi

if [ -d "$DATA" ]; then
	say ""
	say "Kept, because it is the only copy of your snapshots:"
	say "  $DATA ($(du -sh "$DATA" | awk '{print $1}'))"
	say "Remove it too with: $0 --purge"
fi
