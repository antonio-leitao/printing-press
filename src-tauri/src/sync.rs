//! From a line of source to the place in the PDF it became.
//!
//! The other half of `peek`. ⌘click asks SyncTeX which line made a point on
//! the page; this asks which point a line made, so the editor can bring the
//! PDF to wherever its cursor is. The same SyncTeX data answers both, published
//! beside the PDF by the build, and the same `synctex` that shipped with the
//! TeX that wrote it reads it.
//!
//! A markdown line is carried forward the way peek carries a click back: the
//! anchors stored with the build name the line of pandoc's output a block of
//! markdown became, and that is the line SyncTeX is asked about.

use std::path::Path;

use crate::{
    anchors,
    error::{AppError, AppResult},
    model::{DocumentKind, PdfSpot, Project},
    toolchain::{augmented_path, resolve_executable},
};

/// Where `line` of `file` ended up in `pdf`. A line that set nothing — the
/// preamble, a comment — is answered with the nearest one that did, which is
/// SyncTeX's own answer; `None` only when there is no answer at all, like a
/// markdown line above the first block.
///
/// `work_directory` is where the build ran pandoc, for a markdown document:
/// the generated LaTeX is there, under the name SyncTeX recorded.
pub fn forward(
    project: &Project,
    pdf: &Path,
    work_directory: &Path,
    file: &Path,
    line: u32,
) -> AppResult<Option<PdfSpot>> {
    let (input, line) = match project.kind() {
        DocumentKind::Latex => (file.to_path_buf(), line),
        DocumentKind::Markdown => {
            let stored = crate::peek::read_sidecar(pdf, "lines").unwrap_or_default();
            let Some(generated) = anchors::generated_line(&anchors::decode(&stored), line)
            else {
                return Ok(None);
            };
            (work_directory.join(format!("{}.tex", project.job_name())), generated)
        }
    };
    ask_synctex(pdf, &input, line)
}

fn ask_synctex(pdf: &Path, input: &Path, line: u32) -> AppResult<Option<PdfSpot>> {
    let synctex = resolve_executable("synctex").ok_or_else(|| {
        AppError::ToolUnavailable(
            "synctex was not found. It ships with TeX distributions, beside latexmk.".into(),
        )
    })?;
    let output = std::process::Command::new(&synctex)
        .env("PATH", augmented_path(&synctex))
        .arg("view")
        .arg("-i")
        .arg(format!("{line}:0:{}", input.display()))
        .arg("-o")
        .arg(pdf)
        .output()
        .map_err(|error| AppError::Build(format!("could not run synctex: {error}")))?;
    if !output.status.success() {
        return Ok(None);
    }
    Ok(parse(&String::from_utf8_lossy(&output.stdout)))
}

/// One box SyncTeX named: its page, its left edge and baseline, and its size.
#[derive(Default)]
struct Record {
    page: usize,
    h: f32,
    v: f32,
    width: f32,
    height: f32,
}

/// The first box SyncTeX names that has a size.
///
/// One line usually becomes several records: the line of text it set, and
/// zero-sized markers for where a paragraph or a page began. The line of text
/// is the one worth showing, and a marker only stands in when there is no
/// line. `h` and `v` are a box's left edge and its baseline and `H` how far it
/// rises above that, so its top is `v - H`.
fn parse(output: &str) -> Option<PdfSpot> {
    let mut records: Vec<Record> = Vec::new();
    for line in output.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        // Every record opens with the PDF it is about.
        if key == "Output" {
            records.push(Record::default());
            continue;
        }
        let Some(record) = records.last_mut() else {
            continue;
        };
        let value = value.trim();
        match key {
            "Page" => record.page = value.parse().unwrap_or(0),
            "h" => record.h = value.parse().unwrap_or(0.0),
            "v" => record.v = value.parse().unwrap_or(0.0),
            "W" => record.width = value.parse().unwrap_or(0.0),
            "H" => record.height = value.parse().unwrap_or(0.0),
            _ => {}
        }
    }
    records
        .iter()
        .find(|record| record.width > 0.0 && record.height > 0.0)
        .or_else(|| records.first())
        .filter(|record| record.page > 0)
        .map(|record| PdfSpot {
            page: record.page,
            x: record.h,
            y: record.v - record.height,
            width: record.width,
            height: record.height,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Engine;

    #[test]
    fn takes_the_first_box_with_a_size() {
        let output = "This is SyncTeX command line utility, version 1.5\n\
                      SyncTeX result begin\n\
                      Output:/tmp/paper.pdf\nPage:3\nx:0\ny:0\nh:72\nv:72\nW:0\nH:0\n\
                      Output:/tmp/paper.pdf\nPage:3\nx:100.9\ny:107.8\nh:100.9\nv:107.8\nW:358.6\nH:7.4\n\
                      before:\noffset:-1\nmiddle:\nafter:\nSyncTeX result end\n";
        let spot = parse(output).unwrap();
        assert_eq!(spot.page, 3);
        assert_eq!(spot.x, 100.9);
        assert!((spot.y - 100.4).abs() < 0.01, "the top is the baseline less the height");
        assert_eq!(spot.width, 358.6);
        assert_eq!(parse("SyncTeX result begin\nSyncTeX result end\n"), None);
    }

    /// Compiled for real, and checked against where MuPDF finds the same word
    /// on the page: the box SyncTeX gives for a line has to cover that line,
    /// not the one above or below it.
    #[tokio::test]
    async fn a_line_lands_on_the_words_it_set() {
        if resolve_executable("latexmk").is_none() || resolve_executable("synctex").is_none() {
            eprintln!("skipping: latexmk or synctex is not installed");
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("paper");
        std::fs::create_dir_all(&root).unwrap();
        let mut source = String::from("\\documentclass{article}\n\\begin{document}\n");
        for paragraph in 0..30 {
            source.push_str(&format!("Paragraph {paragraph} of filler text, set on a line of its own.\n\n"));
        }
        source.push_str("A kestrel hovers on this line.\n\n\\end{document}\n");
        std::fs::write(root.join("main.tex"), &source).unwrap();
        let kestrel_line = source
            .lines()
            .position(|line| line.contains("kestrel"))
            .unwrap() as u32
            + 1;

        let project = Project {
            id: 1,
            name: "Paper".into(),
            document_path: root.join("main.tex").to_string_lossy().into_owned(),
            engine: Engine::PdfLatex,
            pinned: false,
            created_at: 0,
            last_opened_at: 0,
        };
        let store = tempfile::tempdir().unwrap();
        let repository =
            crate::database::Repository::open(&store.path().join("press.db")).unwrap();
        let prepared = crate::sources::prepare(
            &project,
            &crate::model::SourceRef::Worktree,
            &repository,
            store.path(),
        )
        .unwrap();
        let work = directory.path().join("work");
        let (_handle, cancel) = crate::runner::CancelHandle::new();
        let outcome = crate::runner::run(
            crate::runner::BuildInputs {
                build_id: 1,
                project: &project,
                source: &prepared,
                work_directory: work.clone(),
                log_path: work.join("last-build.log"),
                artifact_directory: directory.path().join("artifacts"),
                limits: crate::runner::Limits::default(),
            },
            cancel,
            std::sync::Arc::new(crate::runner::PidRegistry::default()),
            std::sync::Arc::new(|_| {}),
        )
        .await
        .unwrap();
        let crate::runner::BuildOutcome::Succeeded { product, .. } = outcome else {
            panic!("the fixture should compile");
        };

        let file = root.join("main.tex").canonicalize().unwrap();
        let spot = forward(&project, &product.pdf_path, &work, &file, kestrel_line)
            .unwrap()
            .expect("a line of text lands somewhere");

        let document = crate::render::open(&product.pdf_path).unwrap();
        let hits = crate::render::search_page(&document, spot.page - 1, "kestrel").unwrap();
        let hit = hits.first().expect("the word is on the page SyncTeX named");
        let middle = hit.y + hit.height / 2.0;
        assert!(
            spot.y <= middle && middle <= spot.y + spot.height,
            "the box {spot:?} covers the line the word is on, {hit:?}"
        );

        // A line that set nothing — the preamble — is answered with the
        // nearest one that did, so the cursor anywhere in the file still
        // brings the PDF somewhere sensible.
        let top = forward(&project, &product.pdf_path, &work, &file, 1).unwrap();
        assert_eq!(top.map(|spot| spot.page), Some(1));
    }
}
