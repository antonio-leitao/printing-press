//! What a preset does, shown rather than described.
//!
//! A frontmatter block is a preamble in disguise, and nobody can read a
//! preamble and say what the page will look like. So the same short document is
//! compiled under each preset and its first page drawn: the choice is made by
//! looking, which is the only way a typographic choice is ever really made.
//!
//! It does a second job for free. YAML that pandoc will not parse, a package
//! that is not installed, a `\renewcommand` with a typo in it — all of them
//! fail here, in front of the person who just typed them, instead of at the
//! next build of a real document. A preset is applied to every markdown
//! document Press compiles, so a broken one breaks all of them at once; being
//! told immediately is worth more here than anywhere else in the application.
//!
//! Addressed by the hash of the preset body, which makes the cache trivial:
//! the same YAML is the same preview, an edited preset is a different file, and
//! nothing has to be invalidated because nothing is ever overwritten. It also
//! means the protocol needs no registry — the digest in the URL *is* the path.
//!
//! Compiled with pdflatex, whatever the projects use. There is no project here
//! to take an engine from, and a preset that needs xelatex will say so through
//! the TeX error rather than by silently looking wrong.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use tokio::process::Command;

use crate::toolchain::{augmented_path, resolve_executable};

/// The document every preset is shown through.
///
/// Deliberately carries no frontmatter of its own. Anything it declared would
/// win over the preset — that is the whole precedence rule — and a sample that
/// quietly overrode the thing it is demonstrating would be worse than no sample
/// at all.
///
/// Long enough to show what a preset actually changes: a heading hierarchy, a
/// measure to judge the margins by, and enough consecutive paragraphs to see
/// leading and indentation. Short enough to stay on one page at a small paper
/// size, because the preview shows the first page and a preset that spills onto
/// a second has nothing more to say in it.
pub const SAMPLE: &str = r"# Lorem Ipsum

Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor
incididunt ut labore et dolore magna aliqua. Ut enim ad minim veniam, quis
nostrud exercitation ullamco laboris nisi ut aliquip ex ea commodo consequat.

## Duis Aute Irure

Excepteur sint occaecat cupidatat non proident, sunt in culpa qui officia
deserunt mollit anim id est laborum. Sed ut perspiciatis unde omnis iste natus
error sit voluptatem accusantium doloremque laudantium.

Nemo enim ipsam voluptatem quia voluptas sit aspernatur aut odit aut fugit, sed
quia consequuntur magni dolores eos qui ratione voluptatem sequi nesciunt.

### Neque Porro

Neque porro quisquam est, qui dolorem ipsum quia dolor sit amet, consectetur,
adipisci velit, sed quia non numquam eius modi tempora incidunt ut labore et
dolore magnam aliquam quaerat voluptatem.

## Quis Autem

Ut enim ad minima veniam, quis nostrum exercitationem ullam corporis suscipit
laboriosam, nisi ut aliquid ex ea commodi consequatur.
";

/// The name every preview compiles under, so the output path is known before
/// latexmk has run.
const JOB: &str = "preview";

/// A preset's address: the hash of its body. An empty preset is a preset — it
/// is what "None" shows — so it hashes like any other.
pub fn digest(body: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(body.as_bytes());
    hasher
        .finalize()
        .iter()
        .fold(String::with_capacity(64), |mut out, byte| {
            use std::fmt::Write as _;
            let _ = write!(out, "{byte:02x}");
            out
        })
}

/// Whether a string is one of our digests, and therefore safe to put in a path.
///
/// The digest arrives from the webview in a URL. Sixty-four hex characters
/// cannot contain a separator or a `..`, so checking the shape is what keeps
/// the route from being a way to name arbitrary files.
pub fn is_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Where a compiled preview lives once it exists.
pub fn pdf_path(root: &Path, digest: &str) -> PathBuf {
    root.join(format!("{digest}.pdf"))
}

/// Compiles the sample under a preset, or says why it could not.
///
/// `Err` carries a line fit to show under the field the reader is typing in —
/// pandoc's complaint, or TeX's, whichever came first.
pub async fn compile(root: &Path, body: &str) -> Result<PathBuf, String> {
    let digest = digest(body);
    let pdf = pdf_path(root, &digest);
    // Content-addressed, so an existing file is by definition this preset's.
    if pdf.is_file() {
        return Ok(pdf);
    }

    let pandoc = resolve_executable("pandoc")
        .ok_or_else(|| "pandoc was not found. Install pandoc to preview a preset.".to_owned())?;
    let latexmk = resolve_executable("latexmk")
        .ok_or_else(|| "latexmk was not found. Install a TeX distribution.".to_owned())?;

    // A scratch directory per *compile* rather than per preset. Two cards can
    // ask for the same preview at the same moment — a newly added preset is
    // empty, and an empty preset is the same document as "None" — and sharing a
    // directory would have them overwriting each other's auxiliary files and
    // racing for the same output. Dropped at the end of this function however
    // it returns, so a failure leaves nothing behind either.
    let scratch = tempfile::Builder::new()
        .prefix("compiling-")
        .tempdir_in(root)
        .map_err(|error| format!("could not prepare a preview directory: {error}"))?;
    let work = scratch.path();

    let markdown = work.join("sample.md");
    let generated = work.join(format!("{JOB}.tex"));
    tokio::fs::write(&markdown, SAMPLE)
        .await
        .map_err(|error| format!("could not write the sample: {error}"))?;

    // Prepared exactly as a real build prepares it, or the preview would be
    // showing something other than what the preset does.
    let prepared = if body.trim().is_empty() {
        // Empty is not nothing: it is "None", and None must show pandoc's own
        // default template rather than a metadata file with nothing in it.
        None
    } else {
        Some(crate::frontmatter::prepare(&pandoc, work, JOB, body).await?)
    };

    let mut convert = Command::new(&pandoc);
    convert.current_dir(work);
    convert.env("PATH", augmented_path(&pandoc));
    convert.args(["--from", "markdown", "--to", "latex", "--standalone"]);
    if let Some(prepared) = &prepared {
        convert.arg("--metadata-file").arg(&prepared.metadata);
        if let Some(template) = &prepared.template {
            convert.arg("--template").arg(template);
        }
    }
    convert.arg("--output").arg(&generated);
    convert.arg(&markdown);
    convert.stdin(std::process::Stdio::null());
    let converted = convert
        .output()
        .await
        .map_err(|error| format!("could not start pandoc: {error}"))?;
    if !converted.status.success() {
        return Err(first_complaint(
            &converted.stderr,
            "pandoc could not read this preset",
        ));
    }

    let mut build = Command::new(&latexmk);
    build.current_dir(work);
    build.env("PATH", augmented_path(&latexmk));
    build.env("max_print_line", "1000");
    build.args([
        "-pdf",
        "-interaction=nonstopmode",
        "-halt-on-error",
        "-file-line-error",
    ]);
    build.arg(format!("-jobname={JOB}"));
    build.arg(format!("-outdir={}", work.display()));
    build.arg(&generated);
    build.stdin(std::process::Stdio::null());
    let built = build
        .output()
        .await
        .map_err(|error| format!("could not start latexmk: {error}"))?;

    let produced = work.join(format!("{JOB}.pdf"));
    if !built.status.success() || !produced.is_file() {
        // TeX writes its errors to stdout, not stderr.
        return Err(tex_complaint(&built.stdout));
    }

    // Published under the digest, and the scratch goes with `scratch`: what is
    // kept is one PDF per distinct preset, not a build tree per edit. Renaming
    // over an existing file is fine — a racing compile of the same preset
    // produced the same bytes.
    tokio::fs::rename(&produced, &pdf)
        .await
        .map_err(|error| format!("could not store the preview: {error}"))?;
    Ok(pdf)
}

fn first_complaint(stderr: &[u8], fallback: &str) -> String {
    String::from_utf8_lossy(stderr)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or(fallback)
        .chars()
        .take(300)
        .collect()
}

/// TeX's first real error. `-file-line-error` puts them in `file:line: message`
/// form, and the message is the part worth showing — the file is always the
/// generated sample and the line is a line of it, neither of which the reader
/// has.
fn tex_complaint(stdout: &[u8]) -> String {
    let text = String::from_utf8_lossy(stdout);
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix('!') {
            let message = rest.trim();
            if !message.is_empty() {
                return message.chars().take(300).collect();
            }
        }
        // `file:line: message`, which is what `-file-line-error` produces.
        if let Some((head, message)) = line.split_once(": ")
            && head.contains(".tex:")
            && !message.trim().is_empty()
        {
            return message.trim().chars().take(300).collect();
        }
    }
    "this preset did not compile".to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sample_declares_no_frontmatter_of_its_own() {
        // The whole point of the sample is to be overridden. A `---` at the top
        // would take precedence over the preset it exists to demonstrate.
        assert!(!SAMPLE.trim_start().starts_with("---"));
        assert!(SAMPLE.contains("# Lorem Ipsum"), "a heading");
        assert!(SAMPLE.contains("\n## "), "a section");
        assert!(SAMPLE.contains("\n### "), "and a subsection");
    }

    #[test]
    fn a_digest_addresses_a_body_and_nothing_else() {
        assert_eq!(digest("geometry: a4paper"), digest("geometry: a4paper"));
        assert_ne!(digest("geometry: a4paper"), digest("geometry: letterpaper"));
        // "None" is a preset like any other, and has a preview like any other.
        assert!(is_digest(&digest("")));
    }

    #[test]
    fn only_a_digest_can_name_a_preview() {
        assert!(is_digest(&digest("anything")));
        assert!(!is_digest(""));
        assert!(!is_digest("../../../etc/passwd"));
        assert!(!is_digest(&"a".repeat(63)));
        assert!(!is_digest(&"g".repeat(64)), "hex only");
        // The shape check is what keeps a digest from naming a directory.
        assert!(!pdf_path(Path::new("/cache"), &digest("x"))
            .to_string_lossy()
            .contains(".."));
    }

    #[tokio::test]
    async fn a_preset_that_does_not_compile_says_why() {
        if resolve_executable("pandoc").is_none() || resolve_executable("latexmk").is_none() {
            eprintln!("skipping: pandoc or latexmk is not installed");
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let error = compile(
            directory.path(),
            "header-includes:\n  - \\usepackage{a-package-that-is-not-installed}\n",
        )
        .await
        .expect_err("a missing package cannot compile");
        assert!(!error.is_empty());
        // Nothing is left behind by a failure — not the scratch directory, and
        // certainly not a PDF that would be served as this preset's.
        let leftovers = std::fs::read_dir(directory.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(
            leftovers.is_empty(),
            "a failed preview leaves nothing behind, found {leftovers:?}"
        );
    }

    /// Two cards can ask for the same preview at once: a newly added preset is
    /// empty, and an empty preset is the same document as "None". They must not
    /// compile into the same directory.
    #[tokio::test]
    async fn the_same_preset_can_be_compiled_twice_at_once() {
        if resolve_executable("pandoc").is_none() || resolve_executable("latexmk").is_none() {
            eprintln!("skipping: pandoc or latexmk is not installed");
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let (first, second) = tokio::join!(compile(root, ""), compile(root, ""));
        let first = first.expect("an empty preset is what None shows");
        let second = second.expect("and asking for it twice is not a conflict");
        assert_eq!(first, second);
        assert!(first.is_file());

        // One PDF, and no scratch directory outliving the compiles.
        let kept = std::fs::read_dir(root)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(kept, vec![format!("{}.pdf", digest(""))], "{kept:?}");
    }

    #[tokio::test]
    async fn a_preset_compiles_to_one_page_and_is_cached() {
        if resolve_executable("pandoc").is_none() || resolve_executable("latexmk").is_none() {
            eprintln!("skipping: pandoc or latexmk is not installed");
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        const BODY: &str = "geometry: paperwidth=5.5in, paperheight=8in\n";

        let pdf = compile(directory.path(), BODY).await.unwrap();
        assert!(pdf.is_file());
        let document = crate::render::open(&pdf).unwrap();
        assert_eq!(
            crate::render::page_count(&document).unwrap(),
            1,
            "the sample is meant to stay on one page"
        );
        let geometry = crate::render::geometry(&document).unwrap();
        // 5.5in x 8in in points, which is the preset doing its work.
        assert!((geometry[0].width - 396.0).abs() < 1.0, "{:?}", geometry[0]);
        assert!((geometry[0].height - 576.0).abs() < 1.0, "{:?}", geometry[0]);

        // Asked again, the same bytes come back without another TeX run.
        let stamp = std::fs::metadata(&pdf).unwrap().modified().unwrap();
        let again = compile(directory.path(), BODY).await.unwrap();
        assert_eq!(again, pdf);
        assert_eq!(std::fs::metadata(&again).unwrap().modified().unwrap(), stamp);
    }
}
