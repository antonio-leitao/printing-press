# Press

Press is a Tauri desktop app that keeps LaTeX source trees clean while providing a cached PDF
viewer and a Neovim workflow. Neovim remains the editor; Press owns the PDF, the compiler, and
(eventually) the history.

The backend is built around a source reference — the working tree today, a snapshot later — so
that builds, caches, database rows and events all already carry the dimension the history needs.

## Included

- Add a folder, and Press finds the LaTeX document in it; or add a file, which makes that one
  document a project. A project is keyed on `(folder, main file)`, so several documents can share
  a directory — two markdown essays side by side, or a paper and its poster.
- Bounded, recursive main-file discovery using `\documentclass`, `\begin{document}`, conventional
  filenames, folder depth, `% !TEX root` evidence, and an `\input`/`\include`/`\subimport` graph
  that demotes files which are part of another document.
- A disambiguation picker when there is no safe choice, listing the evidence for each candidate.
- TeX engine detection from `% !TEX program`, overridable per project.
- Markdown as well as LaTeX, opened by naming the file rather than by guessing at a folder. A
  markdown document is a project like any other, so snapshots, the history, the watcher and the
  viewer all work on it unchanged.
- Rust-owned SQLite persistence. One schema, no migrations: Press has a single user, and a stale
  database is cheaper to replace than to migrate. A database from another schema is set aside
  under a `.schemaN.old` name and a fresh one started, so a schema change never stops the
  application from opening and never destroys what was there.
- Build state and published PDFs keyed on `(project, source reference, engine)`, so one project
  can hold many versions at once and a cached build never needs invalidating.
- Press's own history of a project, stored outside the project folder and independent of any
  version control the user already has.
- A build queue: several versions can compile concurrently while the interface stays live.
  Duplicate requests for the same version coalesce instead of piling up.
- Debounced, cancellable builds in a warm cache outside the source tree, one scratch directory
  per version.
- A finished build always publishes, even if the source changed while it ran. Discarding it
  would mean a document that saves faster than it compiles never updates.
- Structured diagnostics parsed from the `.log` file — file, line, severity, message — rather
  than a formatted string, ready for Neovim's quickfix list.
- Real build progress parsed from latexmk's own output: rule, pass number, and page count
  against the previous build's page total.
- Native rendering on MuPDF, whose C source is vendored in the `mupdf` crate, so nothing enters
  the build from outside crates.io. Pages are rasterised on a small pool of threads that own the
  documents, and travel to the webview as raw RGBA over a `press:` URI scheme — no PNG encode, no
  image decode. About 9ms per page warm, off the UI thread.
- Zathura's keymap: `j`/`k`, `h`/`l`, `d`/`u`, `f`/`b`/space, `J`/`K`, `gg`/`G`, counts such as
  `12G`, `+`/`-`/`0`, `a` fit page, `s` fit width. Scrolling glides, and repeated keys add to the
  glide already in flight.
- The viewer never blanks: a new PDF replaces the old one only once its geometry has arrived, and
  a failed load leaves the last good document on screen.
- Bounded viewer memory: page bitmaps are released once a page scrolls well clear of the
  viewport, and cached documents are evicted against a byte budget.
- Full project CRUD, including rename, engine and main-file changes, and removal.
- A **Launch Neovim** action that reuses one stable socket per project and supports Alacritty,
  kitty, Ghostty and WezTerm.
- Opening from the editor: `press <path>` opens whatever project that file belongs to. A second
  launch does not start a second Press — its arguments are handed to the running instance, which
  raises its window.
- Startup sweep of interrupted staging files, unreferenced PDFs and storage belonging to deleted
  projects.

## Requirements

- macOS for the currently tested desktop integration
- A TeX distribution containing `latexmk` in `PATH` or a standard macOS location
- pandoc, for markdown projects only
- Neovim and one of the supported terminals
- Node.js and Rust for development

MuPDF is AGPL-3.0. That binds Press only if it is distributed.

Press deliberately does not enable `--shell-escape`. A `.latexmkrc` is still executable Perl, and
latexmk loads one from its working directory as well as the project root, so every such file
found in a folder is reported before the project is added. Only add folders you trust.

## Development

```sh
npm install
npm run check
npm run test:frontend
cargo test --manifest-path src-tauri/Cargo.toml
npm run tauri dev
```

To build and install the app into `/Applications`:

```sh
npm run release
```

`scripts/install-app.sh` quits any running Press before replacing the bundle, and removes the old
one rather than copying over it. Both matter: `cp -R` onto an existing bundle merges into it, so a
file dropped between versions is left behind, and replacing the bundle under a running process
leaves that process with a signature that no longer resolves — after which macOS refuses it access
to protected folders, and the app keeps running while quietly failing at everything.

The two build tests in `src-tauri/src/runner.rs` compile real documents and are skipped when
`latexmk` is not installed.

## Backend boundaries

The Svelte webview can open the native folder picker and invoke a small set of typed commands. It
has no generic filesystem, shell, or SQL access. Rust owns path validation, discovery,
persistence, process execution, cache publication, and lifecycle cleanup. PDFs are addressed by
artifact id, and their paths are checked to be inside Press-managed storage before any bytes are
read.

SQLite and successful PDFs live in Tauri's application-data directory. Reusable LaTeX auxiliary
files and the latest build log live in the application-cache directory. Only an explicit future
export feature will write generated files into a source project.

## History

`⌘K` stores the project's source under a title. Deliberate and titled, never automatic: the
editor's undo already covers keystrokes, and a history worth reading is one where every entry was
meant. The sidebar lists the working tree pinned at the top, then each version with its age and
whether it is built, unbuilt, or fails to compile. Selecting a version shows it and builds it in
the background if it has never been compiled.

It is **not git**, and deliberately so — Press is not a client for the user's repository, and
branches, merges and remotes have no meaning here. `snapshot.rs` is a content-addressed store:
files are named by the hash of their contents, so a hundred versions of a thesis whose figures
never change store those figures once. A version's `revision` is the hash of its manifest, so two
snapshots of identical content share a revision and therefore share one cached build.

Only project source is stored. `files.rs` holds the single set of rules that both the watcher and
the snapshot store use, so "worth rebuilding for" and "worth keeping" cannot drift apart. Build
output, editor scratch files and anyone else's `.git` are skipped. **Nothing is ever written to
the project folder**; a version is compiled from a temporary checkout that is removed with the
build.

Discarding a version drops its build, and file contents no longer referenced by any version are
swept at the next start.

## Markdown

**Markdown is never inferred from a folder, only ever from being named.** LaTeX files carry
evidence of being a document root — `\documentclass`, `\begin{document}`, a `% !TEX root`
directive, an inclusion graph that tells parts from wholes. Markdown carries none of it, so
ranking the markdown files in a folder would be invention dressed up as discovery, and it was
unreliable in exactly the way invention is. The one reliable signal is the user naming the file,
which is what `:Press` from a markdown buffer does.

So: `:Press` in a markdown buffer compiles that file, and **Add file** in the library does the
same from the app. Everything else — a folder, a directory argument, the folder picker — looks for
LaTeX only, and when it finds none it says so and says that a document is opened from the file
itself.

Because a markdown project is a document rather than a folder, two things follow. Its history
holds that document and the assets beside it, not the other markdown files in the same directory —
those are other projects. And a save to one of those neighbours does not rebuild it, while a save
to a shared image does.

`pandoc --pdf-engine` is already two stages: markdown to LaTeX, then a TeX run. Press runs the
stages itself — pandoc writes `<work>/<jobname>.tex`, latexmk builds it — because pandoc driving
the PDF discards latexmk's auxiliary files on every invocation, and the TeX run is what a build
actually costs. Doing it this way also reuses the existing progress parsing, `.log` diagnostics
and publish path rather than adding a second pipeline beside them.

latexmk runs from the source directory even though its input is in the work directory, so
`\includegraphics` and other relative paths resolve against the folder the author wrote in. The
generated `.tex` is only rewritten when its contents changed, so an unedited rebuild leaves
latexmk's cache intact. Nothing is written to the project folder.

YAML frontmatter reaches the LaTeX through pandoc's `--standalone` template, which is how `title`,
`author`, `documentclass` and `geometry` take effect. Per-project pandoc arguments and templates
are not configurable yet.

One honest limitation: diagnostics from a markdown build name the markdown file but carry no line
number. The numbers in the log refer to pandoc's generated LaTeX, and there is no map back, so a
line number would point somewhere plausible and wrong.

## Opening from Neovim

`:Press` in the companion plugin hands Press the path of the file being edited and nothing else.
Working out which project that file belongs to is Press's job: `intake.rs` matches the path
against known projects first, and otherwise walks up running the same discovery the folder picker
uses, until a document root appears or a repository boundary stops it. Editing
`chapters/three.tex` opens the paper; a paper inside a large repository opens as the paper, not
the repository.

The transport is `tauri-plugin-single-instance`, which was already in place. Launching the binary
a second time delivers its arguments to the instance already running, so there is no socket, no
port and no URL scheme to register. The resolved request is held rather than only emitted, because
a launch from the editor arrives before the webview is listening; the interface collects it on
mount, and taking it clears it so nothing opens twice.

## Deliberately deferred

- Side-by-side version tabs
- Text selection and in-document search, both of which the renderer already supplies: MuPDF
  returns word boxes in about 1ms per page and searches with its own engine
- Neovim RPC diagnostics and quickfix synchronization
- Configurable compiler arguments
- Export/print
- Semantic scroll anchoring across changed pagination
- Polished visual design
