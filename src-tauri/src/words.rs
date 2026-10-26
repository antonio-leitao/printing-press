//! How many words a document has.
//!
//! Counted from the source, not from the PDF. A PDF's words include the running
//! heads, the page numbers, the bibliography and whatever a package printed,
//! none of which is what "how much have I written" is asking about.
//!
//! LaTeX goes to texcount, which ships with every TeX distribution beside
//! latexmk. It reads the source the way TeX does — the preamble, commands and
//! maths are not words — and follows `\input` and `\include` into chapters.
//! Markdown is counted here: the prose, without the front matter and without
//! code blocks.
//!
//! A count is never worth failing anything over. A document whose words cannot
//! be counted is a document without a count.

use std::{path::Path, process::Stdio, time::Duration};

use tokio::process::Command;

use crate::{
    model::{DocumentKind, Project},
    sources::PreparedSource,
    toolchain::{augmented_path, resolve_executable},
};

/// texcount reads a thesis in a few hundred milliseconds. A count that takes
/// longer than this is stuck, not slow.
const COUNT_LIMIT: Duration = Duration::from_secs(20);

/// The words in one version of a document, or `None` when they cannot be
/// counted: texcount missing, the source unreadable, the count stuck.
pub async fn count(project: &Project, source: &PreparedSource) -> Option<i64> {
    match project.kind() {
        DocumentKind::Latex => count_latex(source).await,
        DocumentKind::Markdown => {
            let text = tokio::fs::read_to_string(source.document()).await.ok()?;
            Some(count_markdown(&text))
        }
    }
}

async fn count_latex(source: &PreparedSource) -> Option<i64> {
    let texcount = resolve_executable("texcount")?;
    let mut command = Command::new(&texcount);
    command
        .env("PATH", augmented_path(&texcount))
        .current_dir(&source.directory)
        // One number: the words of the text, the headings and the captions
        // added together, with every included file counted in.
        .args(["-1", "-sum", "-inc", "-q", "-utf8"])
        .arg(Path::new(&source.file_name))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let output = tokio::time::timeout(COUNT_LIMIT, command.output())
        .await
        .ok()?
        .ok()?;
    parse_texcount(&String::from_utf8_lossy(&output.stdout))
}

/// The number in what texcount printed.
///
/// `-1` asks for one line holding the count, but a file it could not find is
/// reported on a line of its own before it, and the count line may carry an
/// error tally after the number. Neither stops the number being the count of
/// everything it could read.
fn parse_texcount(output: &str) -> Option<i64> {
    output.lines().find_map(|line| {
        let digits = line
            .trim_start()
            .chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>();
        digits.parse().ok()
    })
}

/// Words in markdown prose: anything between spaces that has a letter or a
/// digit in it, outside the front matter, fenced code and HTML comments.
///
/// Markup is mostly punctuation, which this leaves out by itself — a `#`, a
/// `-` or a `>` on its own is not a word. A link counts once, which is what it
/// reads as.
pub fn count_markdown(text: &str) -> i64 {
    let mut words = 0;
    let mut fence: Option<&str> = None;
    let mut in_front_matter = false;
    let mut in_comment = false;

    for (index, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if index == 0 && trimmed == "---" {
            in_front_matter = true;
            continue;
        }
        if in_front_matter {
            if trimmed == "---" || trimmed == "..." {
                in_front_matter = false;
            }
            continue;
        }
        if let Some(marker) = fence {
            if trimmed.starts_with(marker) {
                fence = None;
            }
            continue;
        }
        if trimmed.starts_with("```") {
            fence = Some("```");
            continue;
        }
        if trimmed.starts_with("~~~") {
            fence = Some("~~~");
            continue;
        }

        let mut rest = line;
        let mut prose = String::new();
        loop {
            if in_comment {
                match rest.find("-->") {
                    Some(end) => {
                        rest = &rest[end + 3..];
                        in_comment = false;
                    }
                    None => break,
                }
            } else {
                match rest.find("<!--") {
                    Some(start) => {
                        prose.push_str(&rest[..start]);
                        prose.push(' ');
                        rest = &rest[start + 4..];
                        in_comment = true;
                    }
                    None => {
                        prose.push_str(rest);
                        break;
                    }
                }
            }
        }
        words += prose
            .split_whitespace()
            .filter(|word| word.chars().any(char::is_alphanumeric))
            .count() as i64;
    }
    words
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Engine, SourceRef};

    #[test]
    fn reads_the_count_whatever_texcount_printed_around_it() {
        assert_eq!(parse_texcount("1234\n"), Some(1234));
        // A missing chapter is reported first, and tallied after the number.
        assert_eq!(
            parse_texcount("ERROR: File nope not found in path [./].\n\n5\n (errors:1)"),
            Some(5)
        );
        assert_eq!(parse_texcount(""), None);
        assert_eq!(parse_texcount("Invalid option\n"), None);
    }

    #[test]
    fn markdown_counts_prose_and_nothing_else() {
        let essay = "---\ntitle: An Essay With Many Words In Its Title\n---\n\n\
                     # Introduction\n\n\
                     Markdown reaches latexmk through [pandoc](https://pandoc.org).\n\n\
                     - one item\n- another, longer item\n\n\
                     ```python\nprint('not prose at all')\n```\n\n\
                     > A quote <!-- with a remark nobody reads --> counts.\n\
                     <!-- a comment\nthat runs over lines -->\n\
                     Olá, ação — 2026!\n";
        // Introduction (1), the sentence (5), the list (2 + 3), the quote (3),
        // and the last line (3): the title, the code and the comments are not
        // prose.
        assert_eq!(count_markdown(essay), 17);
        assert_eq!(count_markdown(""), 0);
        assert_eq!(count_markdown("---\nfront: matter\n---\n"), 0);
    }

    /// texcount, when it is installed, follows the document into the chapters
    /// it includes and leaves the preamble and the commands out.
    #[tokio::test]
    async fn latex_is_counted_across_every_included_file() {
        if resolve_executable("texcount").is_none() {
            eprintln!("skipping: texcount is not installed");
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::create_dir(root.join("chapters")).unwrap();
        std::fs::write(
            root.join("main.tex"),
            "\\documentclass{article}\n\\usepackage{amsmath}\n\\begin{document}\n\
             \\section{Two Words}\nThree more words $x + y = z$.\n\\input{chapters/one}\n\
             \\end{document}\n",
        )
        .unwrap();
        std::fs::write(root.join("chapters/one.tex"), "Four words in here.\n").unwrap();

        let project = Project {
            id: 1,
            name: "Counted".into(),
            document_path: root.join("main.tex").to_string_lossy().into_owned(),
            engine: Engine::PdfLatex,
            pinned: false,
            created_at: 0,
            last_opened_at: 0,
        };
        let store = tempfile::tempdir().unwrap();
        let repository =
            crate::database::Repository::open(&store.path().join("press.db")).unwrap();
        let source =
            crate::sources::prepare(&project, &SourceRef::Worktree, &repository, store.path())
                .unwrap();
        // Two in the heading, three in the text, four in the chapter, and the
        // formula as one — texcount's sum counts an inline formula as a word,
        // which is how word limits usually treat one.
        assert_eq!(count(&project, &source).await, Some(10));
    }
}
