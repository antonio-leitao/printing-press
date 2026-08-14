//! Which of the three tiles Press wears in the Dock.
//!
//! macOS has no interface for this. iOS does — `setAlternateIconName`, the
//! alternates declared in the Info.plist and the system performing the swap —
//! but the Mac never got one, so an application offering a choice has to write
//! the chosen icon onto its own bundle. That is what Finder's "paste a custom
//! icon" does in Get Info, and `NSWorkspace` is the same operation with a
//! function in front of it: an `Icon\r` file at the bundle root, the picture in
//! its resource fork, and a bit set in the directory's Finder information.
//!
//! Written to the bundle rather than set on the running application, which is
//! the other way round it could be done. `NSApplication`'s
//! `applicationIconImage` changes the tile of a process that is already up, so
//! the Dock would show the built-in icon for as long as Press takes to launch
//! and the chosen one after — a seam on every start. The bundle is read before
//! there is a process to ask, so it is the only place a choice can be recorded
//! that the launch itself already honours.
//!
//! Two consequences follow, and both are the price of that.
//!
//! Installing overwrites it. `install-app.sh` replaces the bundle, and a custom
//! icon goes with the bundle it was written on. So the choice lives in the
//! database and is put back at startup: the setting is the record, and the
//! bundle is only where it shows.
//!
//! And a development build has no bundle to write on. `tauri dev` runs the bare
//! binary, which is a file rather than an application as far as Finder is
//! concerned. Choosing there is stored and does nothing visible until the next
//! install — said plainly by `apply`, rather than reported as a failure, since
//! nothing went wrong.

use std::path::{Path, PathBuf};

use crate::error::{AppError, AppResult};

/// Where the choice is stored. Named here, beside the code that applies it.
pub const SETTING: &str = "appearance.icon";

/// The tile the bundle is built with, and therefore what an unset choice means.
/// `make-icons.py` writes this variant to `icons/icon.icns` as well as to
/// `icons/variants/`, so the two agree by construction.
pub const DEFAULT: &str = "green";

/// The tiles, by the name that is stored.
///
/// Compiled in rather than bundled beside the binary. They are wanted at the
/// moment a choice is made and again at every start, and a resource read off
/// disk is one more thing that can be missing from an application whose whole
/// installation story is "copy a folder into place".
const VARIANTS: [(&str, &[u8]); 3] = [
    ("green", include_bytes!("../icons/variants/press-green.icns")),
    ("ink", include_bytes!("../icons/variants/press-ink.icns")),
    ("sheet", include_bytes!("../icons/variants/press-sheet.icns")),
];

/// The stored name, if it names a tile that exists. An unrecognised one reads
/// as the default rather than as an error: a database carried back from a Press
/// that had a fourth variant should open, wearing the icon it actually has.
pub fn resolve(stored: Option<String>) -> String {
    stored
        .filter(|name| icns(name).is_some())
        .unwrap_or_else(|| DEFAULT.to_owned())
}

fn icns(name: &str) -> Option<&'static [u8]> {
    VARIANTS
        .iter()
        .find(|(variant, _)| *variant == name)
        .map(|(_, bytes)| *bytes)
}

/// Rejects a name that is not one of the three, so that a bad value is refused
/// where it arrives rather than stored and puzzled over later.
pub fn validate(name: &str) -> AppResult<()> {
    if icns(name).is_some() {
        return Ok(());
    }
    Err(AppError::InvalidInput(format!(
        "{name} is not one of Press's icons"
    )))
}

/// The `.app` this binary is inside, or `None` when it is not inside one.
///
/// `Contents/MacOS/press` is three levels below the bundle, and the name is
/// checked rather than assumed: a binary run from anywhere else has three
/// ancestors too, and writing an icon onto whichever directory happens to be up
/// there is not a thing to do on the strength of a path shape.
fn bundle() -> Option<PathBuf> {
    let executable = std::env::current_exe().ok()?;
    let bundle = executable.ancestors().nth(3)?;
    (bundle.extension()? == "app").then(|| bundle.to_path_buf())
}

/// Writes a tile onto the bundle, and says whether there was a bundle to write
/// on. `false` is the development case, not a failure.
pub fn apply(name: &str) -> AppResult<bool> {
    let icns = icns(name).ok_or_else(|| {
        AppError::InvalidInput(format!("{name} is not one of Press's icons"))
    })?;
    let Some(bundle) = bundle() else {
        return Ok(false);
    };
    write_icon(&bundle, icns)?;
    Ok(true)
}

#[cfg(target_os = "macos")]
fn write_icon(bundle: &Path, icns: &[u8]) -> AppResult<()> {
    use objc2::AllocAnyThread;
    use objc2_app_kit::{NSImage, NSWorkspace, NSWorkspaceIconCreationOptions};
    use objc2_foundation::{NSData, NSString};

    let data = NSData::with_bytes(icns);
    let image = NSImage::initWithData(NSImage::alloc(), &data)
        .ok_or_else(|| AppError::InvalidInput("that icon is not a picture".to_owned()))?;
    let path = NSString::from_str(&bundle.to_string_lossy());

    // Not marked as needing the main thread, and it is not one of the calls
    // that does: `NSWorkspace` writes a file here rather than touching the
    // screen. The Dock notices on its own.
    let written = NSWorkspace::sharedWorkspace().setIcon_forFile_options(
        Some(&image),
        &path,
        NSWorkspaceIconCreationOptions::empty(),
    );
    if !written {
        return Err(AppError::InvalidInput(format!(
            "macOS refused to write an icon onto {}",
            bundle.display()
        )));
    }
    Ok(())
}

/// Everywhere else there is no Dock and nothing to write on. The setting is
/// still stored, so a database is the same on either platform.
#[cfg(not(target_os = "macos"))]
fn write_icon(_bundle: &Path, _icns: &[u8]) -> AppResult<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_variant_carries_a_real_icns() {
        for (name, bytes) in VARIANTS {
            assert_eq!(&bytes[..4], b"icns", "{name} is not an icns file");
            assert!(bytes.len() > 1024, "{name} is too small to hold an icon");
        }
    }

    #[test]
    fn the_default_is_one_of_the_variants() {
        assert!(icns(DEFAULT).is_some());
    }

    #[test]
    fn an_unknown_name_reads_as_the_default() {
        assert_eq!(resolve(Some("ink".to_owned())), "ink");
        assert_eq!(resolve(None), DEFAULT);
        // A database from a Press that offered something this one does not.
        assert_eq!(resolve(Some("copper".to_owned())), DEFAULT);
    }

    #[test]
    fn only_the_three_are_accepted() {
        assert!(validate("green").is_ok());
        assert!(validate("sheet").is_ok());
        assert!(validate("").is_err());
        assert!(validate("copper").is_err());
    }

    /// The one part of this that is a call into AppKit rather than a decision,
    /// against a directory of its own so that nothing installed is touched.
    /// What a custom icon *is* on macOS is an `Icon\r` file at the root of the
    /// folder carrying it, so that file appearing is the whole of the evidence.
    #[cfg(target_os = "macos")]
    #[test]
    fn writing_an_icon_leaves_the_mark_macos_reads() {
        let directory = tempfile::tempdir().unwrap();
        let bundle = directory.path().join("Nothing.app");
        std::fs::create_dir(&bundle).unwrap();

        write_icon(&bundle, icns("ink").unwrap()).unwrap();

        let marker = bundle.join("Icon\r");
        assert!(marker.exists(), "macOS records a custom icon as Icon\\r");
        // The picture lives in the resource fork rather than in the file, which
        // is why the file itself is empty and why nothing here reads its length.
        let fork = marker.join("..namedfork/rsrc");
        assert!(
            std::fs::metadata(&fork).is_ok_and(|data| data.len() > 1024),
            "the icon itself belongs in the resource fork"
        );
    }
}
