//! The YAML Press hands pandoc for a markdown document that brought none.
//!
//! Markdown has nowhere to put a preamble. A `.tex` file carries its own, so
//! nothing like this is wanted there; pandoc instead *synthesises* a preamble
//! from its default template, and the only way to say anything about the result
//! is metadata. Which means the frontmatter block, which means typing the same
//! twenty lines at the top of every document. This is that block, kept once.
//!
//! What makes it cheap is that pandoc already resolves the conflict Press would
//! otherwise have to. `--metadata-file` is consulted *underneath* the
//! document's own YAML, key by key: a document that says nothing takes the
//! preset entire, a document that sets only a title keeps the preset's
//! typography, and a document that sets `geometry` overrides that one key and
//! no others. So Press holds its horses per key rather than per document, and
//! it does so without a line of merge logic here.
//!
//! Except for one key, and that exception is why this module does more than
//! name a setting. Pandoc *replaces* a list rather than extending it, so a
//! document adding a single `\usepackage` to its own `header-includes` used to
//! lose every line of the preset's — the font, the leading, the lot. That is
//! the wrong way round: a preset is a house style, and adding a package to one
//! document is not a request to abandon it.
//!
//! So the preamble is moved out from under pandoc's feet. Press renames the
//! preset's `header-includes` to a key of its own and gives pandoc a template
//! that emits that key *before* the document's — which pandoc has then left
//! alone, because nothing collided with it. Both survive, the preset's first,
//! and in a TeX preamble the later line is the one that wins. So the preset
//! always applies and a document overrides it by saying so.
//!
//! The renaming is a single top-level key, done as text. Parsing the YAML to
//! move one field would mean carrying a YAML parser to answer a question the
//! shape of the line already answers.

use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
};

use crate::{
    database::Repository, error::AppResult, toolchain::augmented_path,
};

/// Which preset is selected, by id. Absent means none, which is the state Press
/// starts in and the one that makes it behave exactly as it did before presets
/// existed.
pub const SETTING: &str = "markdown.preset";

/// The body of the selected preset, if one is selected and still exists.
///
/// A selection naming a preset that has been deleted reads as none rather than
/// as an error. Nothing is broken in that case — there is simply no longer a
/// preset there — and a build is the wrong place to learn about it.
pub fn selected(repository: &Repository) -> AppResult<Option<String>> {
    let Some(id) = repository.setting(SETTING)? else {
        return Ok(None);
    };
    let Ok(id) = id.parse::<i64>() else {
        return Ok(None);
    };
    Ok(repository
        .preset(id)?
        .map(|preset| preset.body)
        .filter(|body| !body.trim().is_empty()))
}

/// Where a preset's preamble goes so that pandoc will not treat it as the
/// document's to replace.
const PREAMBLE_KEY: &str = "press-preamble";

/// What pandoc's own template calls the document's preamble. The preset's is
/// emitted immediately before this, so the document's comes after and wins.
const PANDOC_LOOP: &str = "$for(header-includes)$";

/// A preset, ready for pandoc.
pub struct Prepared {
    pub metadata: PathBuf,
    /// Only when the preset carries a preamble. Without one there is nothing to
    /// protect from replacement, and the plain call is the one to make.
    pub template: Option<PathBuf>,
}

/// Moves a top-level `header-includes` onto Press's own key.
///
/// Anchored to the start of a line, so a `header-includes` appearing as an
/// indented value or inside a string is left where it is — only a key in the
/// first column is the document's own list.
fn rename_preamble(body: &str) -> (String, bool) {
    let pattern = regex::Regex::new(r"(?m)^header-includes:").expect("a literal pattern");
    let moved = pattern.is_match(body);
    (
        pattern
            .replace_all(body, format!("{PREAMBLE_KEY}:").as_str())
            .into_owned(),
        moved,
    )
}

/// Pandoc's default template with a loop for the preset's preamble ahead of the
/// document's own.
fn splice(default_template: &str) -> Option<String> {
    let at = default_template.find(PANDOC_LOOP)?;
    let ours = format!("$for({PREAMBLE_KEY})$\n${PREAMBLE_KEY}$\n$endfor$\n");
    let mut spliced = String::with_capacity(default_template.len() + ours.len());
    spliced.push_str(&default_template[..at]);
    spliced.push_str(&ours);
    spliced.push_str(&default_template[at..]);
    Some(spliced)
}

/// Writes what pandoc needs to apply a preset without the document being able
/// to replace its preamble.
///
/// A template is only produced when there is a preamble to protect, and a
/// pandoc whose default template does not look as expected falls back to no
/// template at all — the preset still applies, and only the one exception
/// returns.
pub async fn prepare(
    pandoc: &Path,
    work: &Path,
    stem: &str,
    body: &str,
) -> Result<Prepared, String> {
    let (metadata_text, has_preamble) = rename_preamble(body);
    let metadata = work.join(format!("{stem}.preset.yaml"));
    tokio::fs::write(&metadata, &metadata_text)
        .await
        .map_err(|error| format!("could not store the frontmatter preset: {error}"))?;

    if !has_preamble {
        return Ok(Prepared {
            metadata,
            template: None,
        });
    }

    let default = tokio::process::Command::new(pandoc)
        .env("PATH", augmented_path(pandoc))
        .args([OsStr::new("--print-default-template"), OsStr::new("latex")])
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .map_err(|error| format!("could not ask pandoc for its template: {error}"))?;
    let Some(spliced) = String::from_utf8(default.stdout)
        .ok()
        .filter(|_| default.status.success())
        .as_deref()
        .and_then(splice)
    else {
        return Ok(Prepared {
            metadata,
            template: None,
        });
    };

    let template = work.join(format!("{stem}.template.tex"));
    tokio::fs::write(&template, spliced)
        .await
        .map_err(|error| format!("could not store the pandoc template: {error}"))?;
    Ok(Prepared {
        metadata,
        template: Some(template),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repository() -> (tempfile::TempDir, Repository) {
        let directory = tempfile::tempdir().unwrap();
        let repository = Repository::open(&directory.path().join("press.db")).unwrap();
        (directory, repository)
    }

    #[test]
    fn nothing_selected_is_nothing_applied() {
        let (_directory, repository) = repository();
        assert_eq!(selected(&repository).unwrap(), None);
    }

    #[test]
    fn a_selected_preset_is_the_body_it_holds() {
        let (_directory, repository) = repository();
        let preset = repository
            .save_preset(None, "Pocket book", "geometry: paperwidth=5.5in")
            .unwrap();
        repository
            .set_setting(SETTING, &preset.id.to_string())
            .unwrap();
        assert_eq!(
            selected(&repository).unwrap().as_deref(),
            Some("geometry: paperwidth=5.5in")
        );
    }

    #[test]
    fn a_selection_pointing_at_nothing_reads_as_none() {
        let (_directory, repository) = repository();
        let preset = repository.save_preset(None, "Gone", "geometry: a4paper").unwrap();
        repository
            .set_setting(SETTING, &preset.id.to_string())
            .unwrap();
        repository.delete_preset(preset.id).unwrap();
        // Deleted out from under the selection, and a build should not care.
        assert_eq!(selected(&repository).unwrap(), None);
        // The same for a value that was never an id at all.
        repository.set_setting(SETTING, "none").unwrap();
        assert_eq!(selected(&repository).unwrap(), None);
    }

    #[test]
    fn only_a_top_level_preamble_key_is_moved() {
        let (moved, found) = rename_preamble("header-includes:\n  - \\usepackage{a}\n");
        assert!(found);
        assert!(moved.starts_with("press-preamble:"));

        // Indented, so it belongs to something else and is not the document's
        // own list.
        let (untouched, found) = rename_preamble("other:\n  header-includes: no\n");
        assert!(!found);
        assert_eq!(untouched, "other:\n  header-includes: no\n");

        let (plain, found) = rename_preamble("geometry: a4paper\n");
        assert!(!found, "nothing to move means no template is needed");
        assert_eq!(plain, "geometry: a4paper\n");
    }

    #[test]
    fn the_preset_preamble_is_emitted_before_the_documents() {
        let spliced = splice("A\n$for(header-includes)$\n$header-includes$\n$endfor$\nB")
            .expect("pandoc's template has that loop");
        let ours = spliced.find("press-preamble").unwrap();
        let theirs = spliced.find("$for(header-includes)$").unwrap();
        assert!(
            ours < theirs,
            "the preset goes first so the document's own line is the later one, \
             and in a TeX preamble the later line wins"
        );
        // A pandoc whose template is not shaped like that gets no template at
        // all rather than a broken one.
        assert!(splice("nothing like it").is_none());
    }

    /// The behaviour the whole module exists for, through pandoc itself: a
    /// document adding one package to its own `header-includes` used to discard
    /// every line of the preset's.
    #[tokio::test]
    async fn a_document_adding_to_the_preamble_keeps_the_presets() {
        let Some(pandoc) = crate::toolchain::resolve_executable("pandoc") else {
            eprintln!("skipping: pandoc is not installed");
            return;
        };
        let directory = tempfile::tempdir().unwrap();
        let work = directory.path();
        let prepared = prepare(
            &pandoc,
            work,
            "doc",
            "geometry: paperwidth=5.5in\nheader-includes:\n  - \\usepackage{PRESETFONT}\n",
        )
        .await
        .unwrap();
        let template = prepared.template.expect("a preamble needs the template");

        let source = work.join("doc.md");
        std::fs::write(
            &source,
            "---\nheader-includes:\n  - \\usepackage{USERFONT}\n---\n\nText.\n",
        )
        .unwrap();
        let out = work.join("out.tex");
        let status = tokio::process::Command::new(&pandoc)
            .current_dir(work)
            .args(["--from", "markdown", "--to", "latex", "--standalone"])
            .arg("--metadata-file")
            .arg(&prepared.metadata)
            .arg("--template")
            .arg(&template)
            .arg("--output")
            .arg(&out)
            .arg(&source)
            .output()
            .await
            .unwrap();
        assert!(status.status.success(), "{status:?}");

        let latex = std::fs::read_to_string(&out).unwrap();
        let preset = latex.find("PRESETFONT").expect("the preset's preamble survives");
        let user = latex.find("USERFONT").expect("and so does the document's");
        assert!(preset < user, "the document's line comes after, so it wins");
        assert!(
            latex.contains("paperwidth=5.5in"),
            "ordinary keys still apply"
        );
    }

    #[test]
    fn an_empty_body_is_not_worth_a_metadata_file() {
        let (_directory, repository) = repository();
        let preset = repository.save_preset(None, "Blank", "   \n  ").unwrap();
        repository
            .set_setting(SETTING, &preset.id.to_string())
            .unwrap();
        assert_eq!(selected(&repository).unwrap(), None);
    }
}
