# Press

Press is a Tauri desktop app that keeps source trees clean while providing a cached PDF viewer, a
compiler, and a Neovim workflow. Neovim remains the editor; Press owns the PDF, the build, and the
history.

The backend is built around a source reference — the working tree or a snapshot — so that builds,
caches, database rows and events all carry the dimension the history needs.

## A project is a document

One idea does most of the work here: **a project is a document, and its path is its identity.**

Nothing else about a project is stored. The directory latexmk runs in, the tree the watcher
follows, the files a snapshot holds, whether it is markdown, the job name — all of it derives from
the one path. There is no stored folder, because nothing needed one: latexmk wants a working
directory (the document's own), the watcher wants a path to watch (the same), and a snapshot wants
a *set of files*, for which a folder is merely the cheapest description.

Three things follow, and they are the reason the model is worth stating:

- **Documents sharing a folder are separate projects.** Two markdown essays, a paper and its
  supplementary material, a thesis and its poster. Each has its own history and its own build
  cache.
- **Naming a part opens the whole.** `:Press` inside `chapters/three.tex` opens the thesis,
  because a named file resolves to its document root — through `% !TEX root` first, then the
  inclusion graph. A `standalone` figure that some paper `\input`s opens that paper.
- **A directory is never a project, only a place to look for one.** Point Press at a folder and it
  lists the documents it found, known ones first. It never picks for you.

The one layout this gives up: a document that references files *above* its own directory. Its
working tree still builds — latexmk reads the real filesystem — but a snapshot of it would be
incomplete, and says so at build time rather than silently.

## Included

- Point Press at a document — `.tex`, `.ltx`, `.Rnw`, or markdown — and it compiles that document.
  One way in: the Add button, `:Press`, and `press <path>` all resolve a path the same way and get
  the same answer back.
- Document-root resolution from `% !TEX root`, `\documentclass`, and an
  `\input`/`\include`/`\subimport`/`\subimport` graph that tells parts from wholes.
- A picker when a path means more than one document, with the projects Press already knows listed
  first.
- TeX engine detection from `% !TEX program`, overridable per project.
- Markdown and LaTeX on one path. A markdown document is a project like any other, so snapshots,
  the history, the watcher and the viewer all work on it unchanged.
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
  than a formatted string, so the place is separate from what went wrong.
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
- Project rename, engine change and removal. There is no "change the main file": a different
  document is a different project, which is what lets several live in one folder.
- An **Editor** action that runs a command of your choosing — `{file}` is the document and `{dir}`
  its folder. Press spawns it and has nothing further to do with it: the working tree is watched
  either way, so a save rebuilds the document whoever wrote it. Unset, the document is handed to
  the system to open with whatever you already use for that kind of file.
- Opening from the editor: `press <path>` resolves whatever it is handed. A second launch does not
  start a second Press — its arguments are handed to the running instance, which raises its window.
- Startup sweep of interrupted staging files, unreferenced PDFs and storage belonging to deleted
  projects.

## Requirements

- macOS for the currently tested desktop integration
- A TeX distribution containing `latexmk` in `PATH` or a standard macOS location
- pandoc, for markdown projects only
- An editor, if you want the Editor button to open one; any command will do
- Node.js and Rust for development

MuPDF is AGPL-3.0. That binds Press only if it is distributed.

Press deliberately does not enable `--shell-escape`. A `.latexmkrc` is still executable Perl, and
latexmk loads one from the directory it runs in — the document's own — so such a file is reported
before the document is opened for the first time. Only open documents in folders you trust.

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

The Svelte webview can open the native file picker and invoke a small set of typed commands. It
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

Only this document's source is stored. `files.rs` holds the single rule that both the watcher and
the snapshot store use — `belongs_to_project` — so "worth rebuilding for" and "worth keeping"
cannot drift apart. Build output, editor scratch files and anyone else's `.git` are skipped, and
so are the documents belonging to other projects in the same folder: a neighbour's draft is not
this document's version, and saving it does not rebuild this one. Everything else there is shared
and counts for both — a figure, a `.bib`, a chapter one of them `\input`s.

**Nothing is ever written to the project folder**; a version is compiled from a temporary checkout
that is removed with the build.

Discarding a version drops its build, and file contents no longer referenced by any version are
swept at the next start.

## Markdown

Markdown needs no special case any more. It is a document with a path, like any other, and it gets
the same history, watcher, viewer and picker.

The one place the two differ is *listing*. A LaTeX file carries evidence of being a document root
— `\documentclass`, a `% !TEX root` directive, an inclusion graph that tells parts from wholes —
so a directory scan can say which `.tex` files are documents and which are chapters. Markdown
carries none of that, so every markdown file in a directory is listed as a candidate and the user
picks. Listing is not guessing: what was unreliable before was *auto-selecting* one, and Press
still never does that. Only `README`, `CHANGELOG`, `CONTRIBUTING`, `LICENSE` and `AUTHORS` are left
out of a directory listing, and naming one of those directly still compiles it — naming a file is
always the last word.

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
Working out what that path means is Press's job, and `documents.rs` is the only place that answers
it — for `:Press`, for `press <path>`, and for the library's own button.

A named file resolves to its document root: itself if nothing includes it, otherwise the document
that does, found through `% !TEX root` and then the inclusion graph, walking up until a document
appears or a repository boundary stops it. Editing `chapters/three.tex` opens the paper; a paper
inside a large repository opens as the paper, not the repository; `supplementary.tex` beside that
paper opens as itself, because nothing includes it.

`:Press` from an empty buffer sends the working directory, and a directory always opens the
picker: the documents Press already keeps first, then the ones it found.

The transport is `tauri-plugin-single-instance`, which was already in place. Launching the binary
a second time delivers its arguments to the instance already running, so there is no socket, no
port and no URL scheme to register. The resolved request is held rather than only emitted, because
a launch from the editor arrives before the webview is listening; the interface collects it on
mount, and taking it clears it so nothing opens twice.

## Deliberately deferred

- Side-by-side version tabs
- Text selection and in-document search, both of which the renderer already supplies: MuPDF
  returns word boxes in about 1ms per page and searches with its own engine
- Configurable compiler arguments
- Export/print
- Semantic scroll anchoring across changed pagination
- Polished visual design
