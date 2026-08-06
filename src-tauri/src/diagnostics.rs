//! Turning TeX's output into something structured.
//!
//! Two consumers need this: the strip above the PDF, which wants one sentence,
//! and Neovim's quickfix list, which wants file, line, severity and message. A
//! formatted `String` cannot serve the second, so nothing here produces one
//! until the very last step.

use std::{
    collections::HashSet,
    path::{Component, Path, PathBuf},
    sync::LazyLock,
};

use regex::Regex;

use crate::model::{Diagnostic, Severity};

/// Beyond this a quickfix list stops being useful and starts being a wall.
const MAX_DIAGNOSTICS: usize = 200;
/// How far past a `!` line to look for the `l.<n>` echo that carries its line number.
const LINE_LOOKAHEAD: usize = 24;

static FILE_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(.+?):(\d+):\s*(.*)$").unwrap());
static TEX_ERROR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^!\s*(.*)$").unwrap());
static LINE_ECHO: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^l\.(\d+)").unwrap());
static WARNING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:(LaTeX|Package|Class)\s+)?(?:(\S+)\s+)?(?:LaTeX\s+)?Warning:\s*(.*)$").unwrap()
});
static INPUT_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"on input line (\d+)").unwrap());
static OUTPUT_WRITTEN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"Output written on .*?\((\d+) pages?").unwrap());
static LATEXMK_FAILURE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^Latexmk:\s*(?:!!!\s*)?(.*)$").unwrap());
static RULE_NAME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"rule '([^']+)'").unwrap());
static RUN_NUMBER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"Run number (\d+)").unwrap());
/// A page marker on a complete line: the number is finished, so end-of-line ends it.
static SHIPPED_PAGE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[(\d+)(?:[\]{ ]|$)").unwrap());
/// The same on a partial read, where a trailing `[2` may still become `[23`, so
/// only a terminated marker counts.
static SHIPPED_PAGE_TERMINATED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[(\d+)[\]{ ]").unwrap());

const SOURCE_EXTENSIONS: &[&str] = &[
    "tex", "ltx", "sty", "cls", "def", "cfg", "clo", "bib", "bbl", "aux", "fd", "tikz", "pgf",
    "dtx", "ins", "rnw", "lco",
];

#[derive(Debug, Default, PartialEq, Eq)]
pub struct LogAnalysis {
    pub diagnostics: Vec<Diagnostic>,
    /// Taken from `Output written on ... (N pages, ...)`, which is cheaper and
    /// more reliable than parsing the PDF.
    pub page_count: Option<i64>,
    pub produced_no_pages: bool,
}

/// Parses a `.log` file. `directory` is where latexmk ran, which is both what
/// the log's relative paths are relative to and what a diagnostic's path is
/// reported relative to — the document's own folder, so the two cannot differ.
pub fn analyze_log(text: &str, directory: &Path) -> LogAnalysis {
    let lines = text.lines().collect::<Vec<_>>();
    let mut analysis = LogAnalysis::default();
    let mut stack = FileStack::default();
    let mut seen = HashSet::new();

    for (index, line) in lines.iter().enumerate() {
        if let Some(capture) = OUTPUT_WRITTEN.captures(line) {
            analysis.page_count = capture.get(1).and_then(|value| value.as_str().parse().ok());
        }
        if line.contains("No pages of output") {
            analysis.produced_no_pages = true;
        }

        let diagnostic = parse_error_line(line, &lines, index, &stack, directory)
            .or_else(|| parse_warning_line(line, &lines, index, &stack, directory));
        if let Some(diagnostic) = diagnostic
            && seen.insert(diagnostic.clone())
            && analysis.diagnostics.len() < MAX_DIAGNOSTICS
        {
            analysis.diagnostics.push(diagnostic);
        }

        // The stack is updated after parsing so that an error is attributed to
        // the file that was open when it was reported.
        stack.observe(line);
    }

    analysis.diagnostics.sort_by_key(|diagnostic| match diagnostic.severity {
        Severity::Error => 0,
        Severity::Warning => 1,
    });
    analysis
}

fn parse_error_line(
    line: &str,
    lines: &[&str],
    index: usize,
    stack: &FileStack,
    directory: &Path,
) -> Option<Diagnostic> {
    // `-file-line-error` gives the strongest signal: the file and line are on
    // the error line itself.
    if let Some(capture) = FILE_LINE.captures(line) {
        let raw_file = capture.get(1)?.as_str().trim();
        let message = capture.get(3)?.as_str().trim();
        if looks_like_source_path(raw_file) && !message.is_empty() {
            return Some(Diagnostic {
                file: relativize(raw_file, directory),
                line: capture.get(2)?.as_str().parse().ok(),
                severity: Severity::Error,
                message: clean(message),
            });
        }
    }

    let capture = TEX_ERROR.captures(line)?;
    let message = capture.get(1)?.as_str().trim();
    if message.is_empty() {
        return None;
    }
    Some(Diagnostic {
        file: stack.current().and_then(|file| relativize(file, directory)),
        line: lookahead_line(lines, index),
        severity: Severity::Error,
        message: clean(message),
    })
}

fn parse_warning_line(
    line: &str,
    lines: &[&str],
    index: usize,
    stack: &FileStack,
    directory: &Path,
) -> Option<Diagnostic> {
    // Overfull and underfull boxes are typographic notes, not defects. There can
    // be thousands, and they would bury every real problem.
    if line.starts_with("Overfull") || line.starts_with("Underfull") {
        return None;
    }
    let capture = WARNING.captures(line)?;
    let message = capture.get(3)?.as_str().trim();
    if message.is_empty() {
        return None;
    }
    // A warning's line number is often on the following continuation line.
    let line_number = INPUT_LINE
        .captures(line)
        .or_else(|| lines.get(index + 1).and_then(|next| INPUT_LINE.captures(next)))
        .and_then(|capture| capture.get(1)?.as_str().parse().ok());
    let prefix = capture
        .get(2)
        .map(|value| value.as_str())
        .filter(|value| !value.is_empty());
    let message = match prefix {
        Some(package) if !message.starts_with(package) => format!("{package}: {message}"),
        _ => message.to_owned(),
    };
    Some(Diagnostic {
        file: stack.current().and_then(|file| relativize(file, directory)),
        line: line_number,
        severity: Severity::Warning,
        message: clean(&message),
    })
}

fn lookahead_line(lines: &[&str], index: usize) -> Option<u32> {
    lines
        .iter()
        .skip(index + 1)
        .take(LINE_LOOKAHEAD)
        .find_map(|line| LINE_ECHO.captures(line))
        .and_then(|capture| capture.get(1)?.as_str().parse().ok())
}

/// latexmk's own failures never reach the `.log` file, so they are collected
/// separately from its stdout.
pub fn latexmk_failures(output: &str) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let mut seen = HashSet::new();
    for line in output.lines() {
        let Some(capture) = LATEXMK_FAILURE.captures(line.trim()) else {
            continue;
        };
        let Some(message) = capture.get(1).map(|value| value.as_str().trim()) else {
            continue;
        };
        let lowered = message.to_ascii_lowercase();
        let is_failure = lowered.starts_with("failed")
            || lowered.contains("fatal error")
            || lowered.contains("could not find")
            || lowered.contains("failure to make")
            || lowered.contains("gave an error")
            || lowered.contains("did not find");
        if !is_failure || message.is_empty() {
            continue;
        }
        let diagnostic = Diagnostic {
            file: None,
            line: None,
            severity: Severity::Error,
            message: clean(message),
        };
        if seen.insert(diagnostic.clone()) {
            diagnostics.push(diagnostic);
        }
    }
    diagnostics
}

/// The one line shown above the PDF. Prefers a real error over a warning.
pub fn summarize(diagnostics: &[Diagnostic]) -> Option<String> {
    let first = diagnostics
        .iter()
        .find(|diagnostic| diagnostic.severity == Severity::Error)
        .or_else(|| diagnostics.first())?;
    Some(match (&first.file, first.line) {
        (Some(file), Some(line)) => format!("{file}:{line}: {}", first.message),
        (Some(file), None) => format!("{file}: {}", first.message),
        (None, _) => first.message.clone(),
    })
}

fn clean(message: &str) -> String {
    let collapsed = message.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed.chars().take(400).collect()
}

fn looks_like_source_path(value: &str) -> bool {
    if value.is_empty() || value.contains(' ') && !value.contains('/') {
        return false;
    }
    let extension = Path::new(value)
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase);
    match extension {
        Some(extension) => SOURCE_EXTENSIONS.contains(&extension.as_str()),
        None => false,
    }
}

/// Resolves a path from the log to one relative to the project root. Paths that
/// point outside the project (a system class file, say) keep their absolute form
/// so they are still openable.
fn relativize(raw: &str, directory: &Path) -> Option<String> {
    let trimmed = raw.trim().trim_matches('"');
    if trimmed.is_empty() {
        return None;
    }
    let candidate = Path::new(trimmed);
    let joined = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        directory.join(candidate)
    };
    // The file may legitimately not exist — a missing \include is exactly the
    // case that matters — so fall back to lexical normalization.
    let resolved = joined
        .canonicalize()
        .unwrap_or_else(|_| normalize_lexically(&joined));
    let directory = directory
        .canonicalize()
        .unwrap_or_else(|_| directory.to_path_buf());
    match resolved.strip_prefix(&directory) {
        Ok(relative) => Some(portable(relative)),
        Err(_) => Some(portable(&resolved)),
    }
}

fn normalize_lexically(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                normalized.pop();
            }
            Component::CurDir => {}
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

fn portable(path: &Path) -> String {
    path.components()
        .filter_map(|component| component.as_os_str().to_str())
        .collect::<Vec<_>>()
        .join("/")
        .replace("//", "/")
}

/// Tracks TeX's `(file ... )` nesting so a message without an explicit path can
/// still be attributed to the file that was open.
#[derive(Default)]
struct FileStack {
    entries: Vec<Option<String>>,
}

impl FileStack {
    fn observe(&mut self, line: &str) {
        let bytes = line.as_bytes();
        let mut index = 0;
        while index < bytes.len() {
            match bytes[index] {
                b'(' => {
                    let start = index + 1;
                    let mut end = start;
                    while end < bytes.len()
                        && !matches!(bytes[end], b'(' | b')' | b' ' | b'\t' | b'[' | b'{')
                    {
                        end += 1;
                    }
                    let token = &line[start..end];
                    // Every '(' must push something or the stack desynchronizes,
                    // but only real-looking paths are remembered.
                    self.entries.push(
                        looks_like_source_path(token).then(|| token.trim_matches('"').to_owned()),
                    );
                    index = end;
                }
                b')' => {
                    self.entries.pop();
                    index += 1;
                }
                _ => index += 1,
            }
        }
    }

    fn current(&self) -> Option<&str> {
        self.entries
            .iter()
            .rev()
            .find_map(|entry| entry.as_deref())
    }
}

/// Derives real progress from latexmk's and TeX's own chatter, so the banner can
/// say something true instead of spinning.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ProgressParser {
    rule: Option<String>,
    pass: Option<u32>,
    page: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgressSnapshot {
    pub stage: String,
    pub pass: Option<u32>,
    pub page: Option<u32>,
}

impl ProgressParser {
    /// Returns a snapshot only when something actually changed, so callers can
    /// emit an event per change rather than per line.
    pub fn observe(&mut self, line: &str) -> Option<ProgressSnapshot> {
        self.scan(line, true)
    }

    /// For output read so far that has no newline yet. TeX emits page markers
    /// without one, and waiting for the newline would stall the banner; but a
    /// number cut by the read boundary must not be believed.
    pub fn observe_partial(&mut self, text: &str) -> Option<ProgressSnapshot> {
        self.scan(text, false)
    }

    fn scan(&mut self, line: &str, complete: bool) -> Option<ProgressSnapshot> {
        let mut changed = false;

        if let Some(capture) = RULE_NAME.captures(line)
            && let Some(rule) = capture.get(1).map(|value| value.as_str().to_owned())
            && self.rule.as_deref() != Some(rule.as_str())
        {
            self.rule = Some(rule);
            // A new rule restarts page counting.
            self.page = None;
            changed = true;
        }
        if let Some(capture) = RUN_NUMBER.captures(line)
            && let Some(pass) = capture.get(1).and_then(|value| value.as_str().parse().ok())
            && self.pass != Some(pass)
        {
            self.pass = Some(pass);
            self.page = None;
            changed = true;
        }
        // TeX ships pages as `[1]`, `[2{...}]`; the highest on a line wins.
        let pattern = if complete {
            &*SHIPPED_PAGE
        } else {
            &*SHIPPED_PAGE_TERMINATED
        };
        if let Some(page) = pattern
            .captures_iter(line)
            .filter_map(|capture| capture.get(1)?.as_str().parse::<u32>().ok())
            .max()
            && self.page.is_none_or(|current| page > current)
        {
            self.page = Some(page);
            changed = true;
        }

        changed.then(|| self.snapshot())
    }

    pub fn snapshot(&self) -> ProgressSnapshot {
        ProgressSnapshot {
            stage: self.rule.clone().unwrap_or_else(|| "starting".to_owned()),
            pass: self.pass,
            page: self.page,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roots() -> (tempfile::TempDir, PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        (directory, root)
    }

    #[test]
    fn reads_file_and_line_from_file_line_error_output() {
        let (_guard, root) = roots();
        let log = "This is pdfTeX\n./chapters/one.tex:42: Undefined control sequence.\nl.42 \\foo\n";
        let analysis = analyze_log(log, &root);
        assert_eq!(analysis.diagnostics.len(), 1);
        let diagnostic = &analysis.diagnostics[0];
        assert_eq!(diagnostic.file.as_deref(), Some("chapters/one.tex"));
        assert_eq!(diagnostic.line, Some(42));
        assert_eq!(diagnostic.severity, Severity::Error);
        assert_eq!(diagnostic.message, "Undefined control sequence.");
    }

    #[test]
    fn falls_back_to_the_open_file_and_the_line_echo() {
        let (_guard, root) = roots();
        let log = concat!(
            "(./main.tex\n",
            "(./chapters/two.tex\n",
            "! Undefined control sequence.\n",
            "l.17 \\nope\n",
        );
        let analysis = analyze_log(log, &root);
        assert_eq!(analysis.diagnostics.len(), 1);
        assert_eq!(
            analysis.diagnostics[0].file.as_deref(),
            Some("chapters/two.tex")
        );
        assert_eq!(analysis.diagnostics[0].line, Some(17));
    }

    #[test]
    fn closing_parentheses_pop_the_file_stack() {
        let (_guard, root) = roots();
        let log = concat!(
            "(./main.tex\n",
            "(./chapters/two.tex done)\n",
            "! Missing $ inserted.\n",
        );
        let analysis = analyze_log(log, &root);
        assert_eq!(analysis.diagnostics[0].file.as_deref(), Some("main.tex"));
    }

    #[test]
    fn prose_parentheses_do_not_become_files() {
        let (_guard, root) = roots();
        let log = concat!(
            "(./main.tex\n",
            "(see the transcript file for additional information)\n",
            "! Emergency stop.\n",
        );
        let analysis = analyze_log(log, &root);
        assert_eq!(analysis.diagnostics[0].file.as_deref(), Some("main.tex"));
    }

    #[test]
    fn captures_warnings_with_their_input_line() {
        let (_guard, root) = roots();
        let log = concat!(
            "(./main.tex\n",
            "LaTeX Warning: Reference `fig:one' on page 1 undefined on input line 12.\n",
        );
        let analysis = analyze_log(log, &root);
        assert_eq!(analysis.diagnostics.len(), 1);
        assert_eq!(analysis.diagnostics[0].severity, Severity::Warning);
        assert_eq!(analysis.diagnostics[0].line, Some(12));
        assert_eq!(analysis.diagnostics[0].file.as_deref(), Some("main.tex"));
    }

    #[test]
    fn ignores_overfull_and_underfull_boxes() {
        let (_guard, root) = roots();
        let log = concat!(
            "Overfull \\hbox (12.0pt too wide) in paragraph at lines 10--12\n",
            "Underfull \\vbox (badness 10000) has occurred while \\output is active\n",
        );
        assert!(analyze_log(log, &root).diagnostics.is_empty());
    }

    #[test]
    fn errors_sort_ahead_of_warnings() {
        let (_guard, root) = roots();
        let log = concat!(
            "(./main.tex\n",
            "LaTeX Warning: Citation `x' undefined on input line 3.\n",
            "./main.tex:9: Missing $ inserted.\n",
        );
        let analysis = analyze_log(log, &root);
        assert_eq!(analysis.diagnostics[0].severity, Severity::Error);
        assert_eq!(
            summarize(&analysis.diagnostics).as_deref(),
            Some("main.tex:9: Missing $ inserted.")
        );
    }

    #[test]
    fn keeps_paths_outside_the_project_absolute() {
        let (_guard, root) = roots();
        let log = "/usr/local/texlive/tex/latex/base/article.cls:5: Undefined control sequence.\n";
        let analysis = analyze_log(log, &root);
        assert_eq!(
            analysis.diagnostics[0].file.as_deref(),
            Some("/usr/local/texlive/tex/latex/base/article.cls")
        );
    }

    #[test]
    fn resolves_paths_against_the_directory_latexmk_ran_in() {
        let (_guard, root) = roots();
        std::fs::create_dir_all(root.join("chapters")).unwrap();
        std::fs::write(root.join("chapters/one.tex"), "").unwrap();
        let log = "./chapters/one.tex:3: Missing $ inserted.\n";
        let analysis = analyze_log(log, &root);
        assert_eq!(
            analysis.diagnostics[0].file.as_deref(),
            Some("chapters/one.tex")
        );
    }

    #[test]
    fn reads_the_page_count_from_the_output_line() {
        let (_guard, root) = roots();
        let log = "Output written on main.pdf (42 pages, 1234567 bytes).\n";
        let analysis = analyze_log(log, &root);
        assert_eq!(analysis.page_count, Some(42));
        assert!(!analysis.produced_no_pages);

        let empty = analyze_log("No pages of output.\n", &root);
        assert!(empty.produced_no_pages);
        assert_eq!(empty.page_count, None);
    }

    #[test]
    fn identical_repeated_errors_are_reported_once() {
        let (_guard, root) = roots();
        let log = "./main.tex:4: Missing $ inserted.\n./main.tex:4: Missing $ inserted.\n";
        assert_eq!(analyze_log(log, &root).diagnostics.len(), 1);
    }

    #[test]
    fn collects_latexmk_failures_that_never_reach_the_log() {
        let output = concat!(
            "Latexmk: applying rule 'pdflatex'...\n",
            "Latexmk: Could not find file 'missing.tex'\n",
            "Latexmk: Failed to make pdf file\n",
        );
        let diagnostics = latexmk_failures(output);
        assert_eq!(diagnostics.len(), 2);
        assert!(diagnostics.iter().all(|item| item.severity == Severity::Error));
        assert!(diagnostics[1].message.contains("Failed to make"));
    }

    #[test]
    fn tracks_passes_pages_and_rules() {
        let mut parser = ProgressParser::default();
        assert!(parser.observe("Latexmk: applying rule 'pdflatex'...").is_some());
        assert_eq!(parser.snapshot().stage, "pdflatex");

        let snapshot = parser.observe("Run number 1 of rule 'pdflatex'").unwrap();
        assert_eq!(snapshot.pass, Some(1));

        parser.observe("[1] [2] [3{/usr/local/map}]").unwrap();
        assert_eq!(parser.snapshot().page, Some(3));

        // Page numbers never travel backwards inside one pass.
        assert!(parser.observe("[2]").is_none());

        // A new pass restarts the page counter.
        let snapshot = parser.observe("Run number 2 of rule 'pdflatex'").unwrap();
        assert_eq!(snapshot.pass, Some(2));
        assert_eq!(snapshot.page, None);

        let snapshot = parser.observe("Latexmk: applying rule 'biber main'...").unwrap();
        assert_eq!(snapshot.stage, "biber main");
    }

    #[test]
    fn ignores_bracketed_text_that_is_not_a_page() {
        let mut parser = ProgressParser::default();
        assert!(parser.observe("Package foo [draft] loaded").is_none());
        assert_eq!(parser.snapshot().page, None);
    }
}
