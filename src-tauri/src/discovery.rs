use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io::Read,
    path::{Component, Path, PathBuf},
    sync::LazyLock,
};

use regex::Regex;
use walkdir::{DirEntry, WalkDir};

use crate::{
    error::{AppError, AppResult},
    model::{DiscoveryReport, MainCandidate, path_to_string},
    toolchain::detect_toolchain,
};

const MAX_TEX_FILES: usize = 512;
const MAX_SCAN_BYTES: u64 = 256 * 1024;
static ROOT_DIRECTIVE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\s*%\s*!tex\s+root\s*=\s*(.+?)\s*$").unwrap());
static PROGRAM_DIRECTIVE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\s*%\s*!tex\s+(?:ts-)?program\s*=\s*(.+?)\s*$").unwrap());

#[derive(Default)]
struct CandidateEvidence {
    document_class: bool,
    conventional_name: bool,
    root_level: bool,
    referenced_as_root: bool,
}

pub fn inspect(path: &Path) -> AppResult<DiscoveryReport> {
    if !path.is_dir() {
        return Err(AppError::InvalidInput(format!(
            "{} is not a directory",
            path.display()
        )));
    }
    let root = path.canonicalize()?;
    let root_text = path_to_string(&root)
        .ok_or_else(|| AppError::InvalidInput("project path is not valid UTF-8".into()))?;
    let project_name = root
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("LaTeX project")
        .to_owned();

    let mut tex_files = Vec::new();
    let mut scan_error_count = 0;
    for entry in WalkDir::new(&root)
        .follow_links(false)
        .max_depth(16)
        .into_iter()
        .filter_entry(should_visit)
    {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                scan_error_count += 1;
                continue;
            }
        };
        if entry.file_type().is_file()
            && entry
                .path()
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("tex"))
        {
            tex_files.push(entry.into_path());
            if tex_files.len() == MAX_TEX_FILES {
                break;
            }
        }
    }

    let mut evidence: HashMap<PathBuf, CandidateEvidence> = HashMap::new();
    let mut root_references = HashSet::new();
    let mut unreadable_tex_count = 0;
    for file in &tex_files {
        let relative = relative_path(&root, file)?;
        let metadata = match inspect_tex_file(file) {
            Ok(metadata) => metadata,
            Err(_) => {
                unreadable_tex_count += 1;
                continue;
            }
        };
        let item = evidence.entry(relative.clone()).or_default();
        item.document_class = metadata.has_document_class;
        item.root_level = relative.components().count() == 1;
        item.conventional_name = conventional_name(&relative);

        if let Some(root_directive) = metadata.root_directive
            && let Some(resolved) =
                resolve_inside_root(&root, file.parent().unwrap_or(&root), &root_directive)
            && resolved.is_file()
            && resolved
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("tex"))
        {
            root_references.insert(relative_path(&root, &resolved)?);
        }
    }
    for referenced in root_references {
        evidence.entry(referenced).or_default().referenced_as_root = true;
    }

    let mut candidates = evidence
        .into_iter()
        .filter_map(|(path, evidence)| candidate(path, evidence))
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| left.relative_path.cmp(&right.relative_path))
    });

    let recommended_main = recommend(&candidates);
    let requires_selection = recommended_main.is_none() && candidates.len() > 1;
    let has_latexmkrc = root.join(".latexmkrc").is_file() || root.join("latexmkrc").is_file();
    let mut warnings = Vec::new();
    if tex_files.len() == MAX_TEX_FILES {
        warnings.push(format!(
            "Stopped after {MAX_TEX_FILES} TeX files; narrow the project folder if the main file is missing."
        ));
    }
    if scan_error_count > 0 {
        warnings.push(format!(
            "Could not inspect {scan_error_count} project path(s); check folder permissions if the main file is missing."
        ));
    }
    if unreadable_tex_count > 0 {
        warnings.push(format!(
            "Could not read {unreadable_tex_count} TeX file(s); check their permissions if the main file is missing."
        ));
    }
    if tex_files.is_empty() {
        warnings.push("No .tex files were found in this folder.".into());
    } else if candidates.is_empty() {
        warnings.push("TeX files were found, but none appears to be a document root.".into());
    }

    Ok(DiscoveryReport {
        root_path: root_text,
        project_name,
        tex_file_count: tex_files.len(),
        candidates,
        recommended_main,
        requires_selection,
        has_latexmkrc,
        warnings,
        toolchain: detect_toolchain(),
    })
}

pub fn validate_main(root: &Path, relative: &str) -> AppResult<PathBuf> {
    let relative_path = Path::new(relative);
    if relative_path.is_absolute()
        || relative_path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(AppError::InvalidInput(
            "main file must be inside the selected project".into(),
        ));
    }
    let main = root.join(relative_path).canonicalize()?;
    if !main.starts_with(root) || !main.is_file() {
        return Err(AppError::InvalidInput(
            "main file must be an existing file inside the selected project".into(),
        ));
    }
    if !main
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("tex"))
    {
        return Err(AppError::InvalidInput("main file must end in .tex".into()));
    }
    Ok(main)
}

pub fn detect_engine(main: &Path) -> AppResult<&'static str> {
    let metadata = inspect_tex_file(main)?;
    let Some(program) = metadata.program else {
        return Ok("pdflatex");
    };
    let program = program.to_ascii_lowercase();
    if program.contains("xelatex") {
        Ok("xelatex")
    } else if program.contains("lualatex") || program.contains("luatex") {
        Ok("lualatex")
    } else {
        Ok("pdflatex")
    }
}

fn candidate(path: PathBuf, evidence: CandidateEvidence) -> Option<MainCandidate> {
    if !evidence.document_class && !evidence.referenced_as_root {
        return None;
    }
    let mut score = 0;
    let mut reasons = Vec::new();
    if evidence.document_class {
        score += 100;
        reasons.push("contains \\documentclass".into());
    }
    if evidence.referenced_as_root {
        score += 60;
        reasons.push("referenced by a % !TEX root directive".into());
    }
    if evidence.conventional_name {
        score += 30;
        reasons.push("has a conventional main-document name".into());
    }
    if evidence.root_level {
        score += 15;
        reasons.push("is at the project root".into());
    }
    Some(MainCandidate {
        relative_path: portable_path(&path),
        score,
        reasons,
    })
}

fn recommend(candidates: &[MainCandidate]) -> Option<String> {
    match candidates {
        [] => None,
        [only] => Some(only.relative_path.clone()),
        [first, second, ..] if first.score >= second.score + 25 => {
            Some(first.relative_path.clone())
        }
        _ => None,
    }
}

fn conventional_name(path: &Path) -> bool {
    path.file_stem()
        .and_then(|value| value.to_str())
        .is_some_and(|stem| {
            matches!(
                stem.to_ascii_lowercase().as_str(),
                "main" | "thesis" | "dissertation" | "paper" | "report" | "book"
            )
        })
}

struct TexMetadata {
    has_document_class: bool,
    root_directive: Option<String>,
    program: Option<String>,
}

fn inspect_tex_file(path: &Path) -> AppResult<TexMetadata> {
    let file = File::open(path)?;
    let mut bytes = Vec::new();
    file.take(MAX_SCAN_BYTES).read_to_end(&mut bytes)?;
    let text = String::from_utf8_lossy(&bytes);
    let mut has_document_class = false;
    let mut root_directive = None;
    let mut program = None;
    for (index, line) in text.lines().enumerate() {
        let line_number = index + 1;
        if line_number <= 50 {
            let directive_line = line.trim_start_matches('\u{feff}');
            if root_directive.is_none() {
                root_directive = ROOT_DIRECTIVE
                    .captures(directive_line)
                    .and_then(|capture| capture.get(1))
                    .map(|value| trim_quotes(value.as_str().trim()));
            }
            if program.is_none() {
                program = PROGRAM_DIRECTIVE
                    .captures(directive_line)
                    .and_then(|capture| capture.get(1))
                    .map(|value| trim_quotes(value.as_str().trim()));
            }
        }
        let active = strip_tex_comment(line).trim_start_matches(|character: char| {
            character.is_whitespace() || character == '\u{feff}'
        });
        if active.strip_prefix("\\documentclass").is_some_and(|rest| {
            let rest = rest.trim_start();
            rest.starts_with('{') || rest.starts_with('[')
        }) {
            has_document_class = true;
        }
    }
    Ok(TexMetadata {
        has_document_class,
        root_directive,
        program,
    })
}

fn strip_tex_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        if *byte != b'%' {
            continue;
        }
        let mut slashes = 0;
        let mut cursor = index;
        while cursor > 0 && bytes[cursor - 1] == b'\\' {
            slashes += 1;
            cursor -= 1;
        }
        if slashes % 2 == 0 {
            return &line[..index];
        }
    }
    line
}

fn trim_quotes(value: &str) -> String {
    if value.len() >= 2
        && ((value.starts_with('"') && value.ends_with('"'))
            || (value.starts_with('\'') && value.ends_with('\'')))
    {
        value[1..value.len() - 1].to_owned()
    } else {
        value.to_owned()
    }
}

fn resolve_inside_root(root: &Path, base: &Path, value: &str) -> Option<PathBuf> {
    let path = Path::new(value);
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    };
    let canonical = joined.canonicalize().ok()?;
    canonical.starts_with(root).then_some(canonical)
}

fn relative_path(root: &Path, path: &Path) -> AppResult<PathBuf> {
    path.strip_prefix(root)
        .map(Path::to_path_buf)
        .map_err(|_| AppError::InvalidInput("project file escaped the selected folder".into()))
}

fn portable_path(path: &Path) -> String {
    path.components()
        .filter_map(|component| component.as_os_str().to_str())
        .collect::<Vec<_>>()
        .join("/")
}

fn should_visit(entry: &DirEntry) -> bool {
    if entry.depth() == 0 || !entry.file_type().is_dir() {
        return true;
    }
    !matches!(
        entry
            .file_name()
            .to_string_lossy()
            .to_ascii_lowercase()
            .as_str(),
        ".git" | ".svn" | ".hg" | "node_modules" | "target" | "build" | "out" | "dist" | ".cache"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_a_unique_main_and_honors_comments() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join("chapter.tex"),
            "% \\documentclass{book}\nhello",
        )
        .unwrap();
        std::fs::write(directory.path().join("main.tex"), "\\documentclass{book}\n").unwrap();

        let report = inspect(directory.path()).unwrap();
        assert_eq!(report.recommended_main.as_deref(), Some("main.tex"));
        assert_eq!(report.candidates.len(), 1);
    }

    #[test]
    fn root_directive_strengthens_the_referenced_file() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir(directory.path().join("chapters")).unwrap();
        std::fs::write(directory.path().join("main.tex"), "\\documentclass{book}\n").unwrap();
        std::fs::write(
            directory.path().join("chapters/one.tex"),
            "% !TEX root = ../main.tex\ntext",
        )
        .unwrap();

        let report = inspect(directory.path()).unwrap();
        assert_eq!(report.recommended_main.as_deref(), Some("main.tex"));
        assert!(
            report.candidates[0]
                .reasons
                .iter()
                .any(|reason| reason.contains("root directive"))
        );
    }

    #[test]
    fn refuses_main_files_outside_the_project() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("project");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(directory.path().join("outside.tex"), "").unwrap();
        assert!(validate_main(&root, "../outside.tex").is_err());
    }

    #[test]
    fn discovers_ascii_latex_commands_in_non_utf8_sources() {
        let directory = tempfile::tempdir().unwrap();
        let mut source = b"\\documentclass{article}\n% latin-1: ".to_vec();
        source.push(0xff);
        std::fs::write(directory.path().join("main.tex"), source).unwrap();

        let report = inspect(directory.path()).unwrap();
        assert_eq!(report.recommended_main.as_deref(), Some("main.tex"));
    }

    #[test]
    fn detects_bom_and_texshop_engine_directives() {
        let directory = tempfile::tempdir().unwrap();
        let main = directory.path().join("main.tex");
        std::fs::write(
            &main,
            "\u{feff}% !TEX TS-program = xelatex\n\\documentclass{article}\n",
        )
        .unwrap();

        let report = inspect(directory.path()).unwrap();
        assert_eq!(report.recommended_main.as_deref(), Some("main.tex"));
        assert_eq!(detect_engine(&main).unwrap(), "xelatex");
    }
}
