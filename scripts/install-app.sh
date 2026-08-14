#!/bin/sh
# Installs the release bundle into /Applications. Builds nothing: run
# `npm run tauri build` first, or `npm run install:app` to do both.
#
# With --link, also puts a `press` command on your PATH. Off by default: an app
# you built yourself should not quietly add things to your PATH, and press.nvim
# does not need it — it finds the bundle on its own. Asked for once, it stays:
# every later rebuild puts it back without being told again.
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
LINK="$HOME/.local/bin/press"

link=no
for argument in "$@"; do
	case "$argument" in
	--link) link=yes ;;
	*)
		echo "usage: $0 [--link]" >&2
		exit 2
		;;
	esac
done

if [ ! -d "$BUNDLE" ]; then
	echo "No release bundle at $BUNDLE" >&2
	echo "Run 'npm run tauri build' first, or 'npm run install:app' to do both." >&2
	exit 1
fi

# Settled before the uninstall below, because that removes the `press` script
# along with the app — and a rebuild is not a removal. A script already ours
# means --link was asked for once already, so it is asked for still; requiring
# the flag again on every rebuild would make it a thing you have to remember.
#
# This is also the right moment to refuse a file that is not ours: before
# anything has been removed, rather than after the app is already installed.
if [ -e "$LINK" ]; then
	if head -5 "$LINK" 2>/dev/null | grep -q press-cli-shim; then
		link=yes
	elif [ "$link" = yes ]; then
		echo "Something that is not ours is already at $LINK — leaving it alone." >&2
		exit 1
	fi
fi

sh "$root/scripts/uninstall-app.sh" --quiet

# ditto rather than cp: it is the macOS-native copy and keeps extended
# attributes and the signature intact.
ditto "$BUNDLE" "$TARGET"

if [ ! -x "$TARGET/Contents/MacOS/press" ]; then
	echo "Copied $TARGET but its binary is missing or not executable." >&2
	exit 1
fi

# The third thing a plain `cp -R` does not do, and the one that is invisible
# until you change the icon.
#
# LaunchServices and the Dock cache an app's icon against its bundle path, and
# the copy above reuses a path both of them have already seen. Nothing in the
# new bundle tells them to look again, so the old icon survives a reinstall —
# the app is genuinely updated and genuinely shows the wrong icon, which reads
# as the build having silently failed.
#
# `touch` alone is not enough once the icon is in the cache. Re-registering the
# bundle is what actually drops the entry, and the Dock only re-reads what it
# has drawn when it restarts. It comes straight back.
touch "$TARGET"
LSREGISTER=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister
[ -x "$LSREGISTER" ] && "$LSREGISTER" -f "$TARGET" || true
killall Dock 2>/dev/null || true

echo "Installed $TARGET"
codesign -dv "$TARGET" 2>&1 | grep -E "^Identifier" || true
du -sh "$TARGET" | awk '{print "  " $1}'

if [ "$link" = no ]; then
	exit 0
fi

mkdir -p "$(dirname "$LINK")"

# A script rather than a symlink to the binary, for two reasons the launch
# itself proves.
#
# `open` hands the launch to LaunchServices, so the app comes up owned by
# launchd rather than by the shell you typed in — close the terminal and Press
# lives on. A symlink to the binary makes your terminal its parent instead.
#
# And the paths are made absolute first. `open` starts the app with a working
# directory of its own, not yours, so a relative `press paper.tex` would be
# resolved against the wrong folder and quietly open nothing. Absolute paths
# have no folder to be wrong about.
cat >"$LINK" <<'SHIM'
#!/bin/sh
# press-cli-shim — written by Press's install script; safe to remove.
set -eu

APP="/Applications/Press.app"

if [ ! -d "$APP" ]; then
	echo "Press is not installed at $APP." >&2
	echo "Build it with 'npm run install:app' in the Press repository." >&2
	exit 127
fi

if [ "$#" -eq 0 ]; then
	exec open -n -a "$APP"
fi

# Rewrite every path argument as an absolute one, appending as we go and then
# dropping the originals. The app is started by LaunchServices with its own
# working directory, so a relative path would mean something else by the time it
# arrived.
count=$#
for argument in "$@"; do
	case "$argument" in
	-* | /*) ;;
	*) argument="$PWD/$argument" ;;
	esac
	set -- "$@" "$argument"
done
shift "$count"

exec open -n -a "$APP" --args "$@"
SHIM
chmod +x "$LINK"

echo "Linked $LINK"
case ":$PATH:" in
*":$(dirname "$LINK"):"*) ;;
*)
	echo "  $(dirname "$LINK") is not on your PATH, so 'press' will not be found." >&2
	echo "  Add it, or move the file somewhere that is." >&2
	;;
esac
