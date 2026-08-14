//! Opening the document in whatever the reader writes with.
//!
//! One command, spawned and forgotten. Press neither talks to the editor nor
//! listens to it, and that is the design rather than a gap in it: the working
//! tree is watched, so a save rebuilds the document whoever wrote it. The
//! editor is a way of putting bytes on disk, not a part of the build. Nothing
//! here opens a channel, so there is nothing to keep alive, nothing to reconnect
//! and nothing to mind — either window can be closed without consequence for the
//! other.
//!
//! What the reader configures is a command line. Press fills in where the
//! document is and where its folder is, and runs it. A GUI editor is one word
//! and a placeholder; a terminal editor is the terminal, its flags and the
//! editor. Anything that will not fit on one line fits in a script, which is a
//! better place for it than a table inside Press.

use std::{
    path::Path,
    process::{Command, Stdio},
};

use crate::{
    error::{AppError, AppResult},
    model::Project,
    toolchain::{augmented_path, resolve_executable},
};

/// Where the editor command is stored. Named here, beside the code that runs
/// it, so the reader of either finds the other.
pub const SETTING: &str = "editor.command";

/// The document, and the folder it lives in — which is also the folder latexmk
/// builds in, so it is the one an editor should open on.
const FILE: &str = "{file}";
const DIRECTORY: &str = "{dir}";

/// What the button does before anyone has said what it should do: hand the
/// document to the system and let it honour whatever the reader already set as
/// their editor for this kind of file. It needs no configuration, no detection
/// and no list of editors to keep current, and it is right often enough that
/// configuring the command is an improvement rather than a prerequisite.
pub const fn default_command() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "open {file}"
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        "xdg-open {file}"
    }
    #[cfg(windows)]
    {
        "cmd /C start \"\" {file}"
    }
}

/// Runs the reader's command against a document.
///
/// Spawned into its own process group so that quitting Press never takes the
/// editor with it, and reaped on a thread of its own: nothing here wants the
/// exit status, but a child nobody waits for is a zombie until Press exits.
/// Reaping this one child rather than ignoring `SIGCHLD` for the whole process
/// matters — latexmk is waited on, and a process-wide ignore would take its exit
/// status away with it.
pub fn launch(project: &Project, command: &str) -> AppResult<String> {
    let document = project.document();
    let directory = project.directory();

    let words = split(command)?;
    let (program, arguments) = words.split_first().ok_or_else(|| {
        AppError::InvalidInput("The editor command is empty. Set one in Settings.".into())
    })?;

    let program = fill(program, &document, &directory);
    let resolved = resolve_executable(&program).ok_or_else(|| {
        AppError::ToolUnavailable(format!(
            "{program} was not found. Check the editor command in Settings."
        ))
    })?;

    let mut child = Command::new(&resolved);
    for argument in arguments {
        child.arg(fill(argument, &document, &directory));
    }
    child
        // An editor opened on a document should start where the document is,
        // which saves most commands from naming {dir} at all.
        .current_dir(&directory)
        // Press resolves the first word itself, but only the first: a terminal
        // is usually asked to run something else, and `alacritty -e nvim` leaves
        // finding nvim to alacritty. Started from the Dock, Press inherits a
        // PATH with almost nothing on it, so what it hands on has to be the one
        // it searched rather than the one it was given — the same PATH latexmk
        // and pandoc are run with.
        .env("PATH", augmented_path(&resolved))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        child.process_group(0);
    }

    let spawned = child.spawn().map_err(|error| {
        AppError::Build(format!("could not start {}: {error}", resolved.display()))
    })?;
    reap(spawned);

    Ok(format!("Opened {} in {program}.", name_of(&document)))
}

/// Waits for one child somewhere out of the way. The thread lives as long as
/// the editor does and costs a stack; opening an editor is a thing a reader does
/// a handful of times in a session, not a thing in a loop.
fn reap(mut child: std::process::Child) {
    std::thread::spawn(move || {
        let _ = child.wait();
    });
}

fn name_of(document: &Path) -> String {
    document
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| document.to_string_lossy().into_owned())
}

/// Puts the document and its folder into one word of the command.
///
/// After the split, never before: a word is one argument no matter what the
/// substitution puts in it, so a path with a space in it stays one path.
fn fill(word: &str, document: &Path, directory: &Path) -> String {
    word.replace(FILE, &document.to_string_lossy())
        .replace(DIRECTORY, &directory.to_string_lossy())
}

/// Splits a command line the way a shell would — single quotes, double quotes
/// and backslash escapes — without being a shell.
///
/// That distinction is the whole reason this function exists. `{file}` is a path
/// off the reader's own disk, and a document may perfectly legally be called
/// `paper; rm -rf ~.tex`. Building the finished string and handing it to `sh -c`
/// would turn that name into a command. Splitting first and filling the pieces
/// in afterwards means a path is always exactly one argument, whatever is in it.
fn split(command: &str) -> AppResult<Vec<String>> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut started = false;
    let mut quote: Option<char> = None;
    let mut characters = command.chars();

    while let Some(character) = characters.next() {
        match quote {
            // Inside single quotes everything is literal, which is what makes
            // them the way to write a Windows path without doubling every slash.
            Some('\'') => {
                if character == '\'' {
                    quote = None;
                } else {
                    word.push(character);
                }
            }
            Some(_) => match character {
                '"' => quote = None,
                '\\' => {
                    if let Some(escaped) = characters.next() {
                        word.push(escaped);
                    }
                }
                _ => word.push(character),
            },
            None => match character {
                '\'' | '"' => {
                    quote = Some(character);
                    started = true;
                }
                '\\' => {
                    if let Some(escaped) = characters.next() {
                        word.push(escaped);
                        started = true;
                    }
                }
                _ if character.is_whitespace() => {
                    if started {
                        words.push(std::mem::take(&mut word));
                        started = false;
                    }
                }
                _ => {
                    word.push(character);
                    started = true;
                }
            },
        }
    }

    if quote.is_some() {
        return Err(AppError::InvalidInput(
            "The editor command has a quote that is never closed.".into(),
        ));
    }
    if started {
        words.push(word);
    }
    Ok(words)
}

/// A first command for a machine that has never been asked. Offered when the
/// setting is first written and behind the dialog's own reset, so that a reader
/// who edits it is never argued with afterwards.
///
/// Deliberately shallow: it knows one editor and four terminals because that is
/// what a suggestion is worth. Anything not found here falls back to the
/// system's own answer, which is the better default in every case except the one
/// this is for — a terminal editor, which the system cannot open on its own.
pub fn suggested_command() -> String {
    const TERMINALS: &[(&str, &str)] = &[
        ("alacritty", "alacritty --working-directory {dir} -e"),
        ("kitty", "kitty --directory {dir}"),
        ("ghostty", "ghostty --working-directory={dir} -e"),
        ("wezterm", "wezterm start --cwd {dir} --"),
    ];

    if resolve_executable("nvim").is_some()
        && let Some((_, prefix)) = TERMINALS
            .iter()
            .find(|(terminal, _)| resolve_executable(terminal).is_some())
    {
        return format!("{prefix} nvim {FILE}");
    }
    default_command().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Engine;

    fn project(document_path: &str) -> Project {
        Project {
            id: 1,
            name: "Test".into(),
            document_path: document_path.into(),
            engine: Engine::PdfLatex,
            pinned: false,
            created_at: 0,
            last_opened_at: 0,
        }
    }

    #[test]
    fn splits_a_command_the_way_a_shell_would() {
        assert_eq!(split("code {file}").unwrap(), ["code", "{file}"]);
        assert_eq!(
            split("alacritty --working-directory {dir} -e nvim {file}").unwrap(),
            [
                "alacritty",
                "--working-directory",
                "{dir}",
                "-e",
                "nvim",
                "{file}"
            ]
        );
        // Runs of whitespace are one separator, and the ends are trimmed.
        assert_eq!(split("  code   {file}  ").unwrap(), ["code", "{file}"]);
        // A quoted program is one word, which is how a path with a space in it
        // is written.
        assert_eq!(
            split("\"/Applications/My Editor.app/Contents/MacOS/edit\" {file}").unwrap(),
            ["/Applications/My Editor.app/Contents/MacOS/edit", "{file}"]
        );
        assert_eq!(split("'one word' two").unwrap(), ["one word", "two"]);
        assert_eq!(split("a\\ b c").unwrap(), ["a b", "c"]);
        // An empty argument is still an argument: `start ""` needs it.
        assert_eq!(
            split("cmd /C start \"\" {file}").unwrap(),
            ["cmd", "/C", "start", "", "{file}"]
        );
        assert!(split("code \"{file}").is_err());
        assert!(split("   ").unwrap().is_empty());
    }

    /// The reason the command is split before it is filled in, rather than
    /// after. Both of these are legal file names.
    #[test]
    fn a_hostile_file_name_stays_one_argument() {
        let document = Path::new("/papers/drift/paper; rm -rf ~.tex");
        let directory = Path::new("/papers/drift");
        assert_eq!(
            fill("{file}", document, directory),
            "/papers/drift/paper; rm -rf ~.tex"
        );
        let spaced = Path::new("/papers/my drift paper/main.tex");
        assert_eq!(
            fill("{file}", spaced, directory),
            "/papers/my drift paper/main.tex"
        );
        // Whatever the substitution puts in, the word count does not change.
        let words = split("nvim {file}").unwrap();
        assert_eq!(words.len(), 2);
        assert_eq!(
            words
                .iter()
                .map(|word| fill(word, document, directory))
                .collect::<Vec<_>>()
                .len(),
            2
        );
    }

    #[test]
    fn fills_the_document_and_its_folder() {
        let project = project("/papers/drift/main.tex");
        let document = project.document();
        let directory = project.directory();
        assert_eq!(
            fill("{file}", &document, &directory),
            "/papers/drift/main.tex"
        );
        assert_eq!(fill("{dir}", &document, &directory), "/papers/drift");
        // Joined forms, which some terminals need.
        assert_eq!(
            fill("--working-directory={dir}", &document, &directory),
            "--working-directory=/papers/drift"
        );
        // A word with no placeholder is left exactly as it was written.
        assert_eq!(fill("-e", &document, &directory), "-e");
    }

    #[test]
    fn an_empty_command_says_so_rather_than_failing_obscurely() {
        let error = launch(&project("/papers/drift/main.tex"), "   ").unwrap_err();
        assert!(
            format!("{error}").contains("empty"),
            "an empty command explains itself: {error}"
        );
    }

    #[test]
    fn a_command_that_is_not_installed_names_itself() {
        let error = launch(
            &project("/papers/drift/main.tex"),
            "press-no-such-editor {file}",
        )
        .unwrap_err();
        let message = format!("{error}");
        assert!(
            message.contains("press-no-such-editor"),
            "the missing program is named: {message}"
        );
    }

    /// Whatever this machine has on it, the suggestion has to be a command:
    /// something to run, and the document to run it on. A suggestion that does
    /// not split, or that never names the file, would be offered to a reader as
    /// a working default and then fail on the first press.
    #[test]
    fn the_suggested_command_is_runnable_on_this_machine() {
        let suggested = suggested_command();
        let words = split(&suggested).unwrap();
        assert!(words.len() >= 2, "a program and a document: {suggested}");
        assert!(
            words.iter().any(|word| word.contains(FILE)),
            "the document is named: {suggested}"
        );
        assert!(
            resolve_executable(&words[0]).is_some(),
            "only ever suggests something installed: {suggested}"
        );
        eprintln!("suggested on this machine: {suggested}");
    }

    #[test]
    fn the_default_command_is_one_the_system_can_answer() {
        // Whatever the platform, the fallback is a program and a placeholder.
        let words = split(default_command()).unwrap();
        assert!(words.len() >= 2);
        assert!(words.iter().any(|word| word.contains(FILE)));
    }
}
