<p align="center">
  <img src='static/press.svg' height='200px' align="center"></img>
</p>

<div align="center">
<h3 max-width='200px' align="center">Printing Press</h3>
  <p><i>A PDF viewer that does not require a PDF<br/>
  Give it LaTeX or Markdown source — the viewer handles compilation<br/>
  Built with Rust</i><br/></p>
  <p>
    <img alt="macOS" src="https://img.shields.io/badge/macOS-black?style=for-the-badge&logo=apple&logoColor=white">
    <img alt="Rust" src="https://img.shields.io/badge/rust-black?style=for-the-badge&logo=rust&logoColor=white">
    <img alt="AGPL-3.0" src="https://img.shields.io/badge/AGPL--3.0-black?style=for-the-badge">
  </p>
</div>

#

### Contents

- [Installation](#installation)
- [Opening a document](#opening-a-document)
- [Editing](#editing)
  - [Neovim](#neovim)
- [Versions](#versions)
- [Markdown](#markdown)
- [Keys](#keys)
- [Settings](#settings)
- [Requirements](#requirements)
- [Building from source](#building-from-source)
- [Not there yet](#not-there-yet)
- [License](#license)

Press is a PDF viewer for LaTeX and Markdown source files. Give it the source, and it compiles and
displays the document for you.

Working on a document usually involves three tasks: writing the source, compiling it, and viewing
the result. Press changes how those tasks are grouped:

`(write + compile) + view` → `write + (compile + view)`

Compilation becomes part of the viewer rather than a separate step in your workflow. You work on
the source; Press turns it into a document, just as any other viewer renders text on a page.

The rest of Press follows from this idea:

- **Press watches the source.** When you save a file, Press rebuilds the document and updates the
  view. You do not need `latexmk -pvc`, a watcher script, a reload command, or any other program to
  connect the editor and viewer. Press manages the entire compile-and-view loop.
- **The PDF is a view, not a product.** You do not need to name, store, or clean up a generated PDF.
  Press keeps it in its own cache and writes nothing to your project folder. If you remove Press,
  the folder still contains exactly the source files you created.
- **Every saved state can have its own view.** Press uses this model to provide a simple document
  history. Use `⌘K` to save a titled snapshot of the current source. You can view any snapshot
  like the working copy, and Press compiles it when necessary. The source is the document; the PDF
  is only a view of that source at a particular moment.

## Installation

Press is not distributed as a prebuilt download. After cloning the repository, build and install
it with:

```sh
npm install
npm run install:app
```

These commands install `Press.app` in `/Applications`. Run them again whenever you want to
upgrade. The first build takes longer because it compiles the PDF renderer's C code; later builds
are faster.

To add a `press` command to your terminal, run:

```sh
npm run install:app -- --link
```

This writes a small script to `~/.local/bin/press`, allowing you to run `press paper.tex` from any
directory. You only need to use the flag once.

To remove Press:

```sh
npm run uninstall:app            # the app
npm run uninstall:app -- --purge # the app and every version it stored
```

Without `--purge`, Press keeps your document history. Snapshots live in Press's own storage, not in
your project folders. That storage contains the only copy of your saved versions, so purge it only
if you no longer need them.

## Opening a document

You can open a document in any of three ways:

- **Drag** a `.tex` or `.md` file onto the Press window, or use the Add button.
- **`press paper.tex`** from the terminal, if you linked the command.
- **`:Press`** from Neovim.

Give Press a file and it opens the corresponding document. Give it a folder and it lists the
documents it finds, placing documents you have used before at the top. You then choose which one
to open.

You can open a document from any file that belongs to it. For example, running `:Press` while
editing `chapters/three.tex` opens the full thesis. Press follows `% !TEX root` directives and the
`\input` graph to find the document root. If a folder contains two papers, Press treats them as
separate projects, with separate builds and histories.

You only need to add a document once. Press then keeps it in the library, ready to open without
first building or locating a PDF.

## Editing

Press is a viewer, not an editor. The **Editor** button runs the command you choose in Settings.
Use `{file}` as a placeholder for the document and `{dir}` for its folder. If you leave the command
unset, Press opens the file with the system's default application.

Whenever a file changes, Press rebuilds the document. It does not matter whether the change came
from Vim, VS Code, Emacs, TextEdit, or a background script. Because Press watches the files
directly, your editor needs no plugin, save hook, reload command, or shared port. The PDF updates
in place, preserves your scroll position, and remains visible while Press recompiles it.

Click a reference, citation, or link in the PDF to follow it. `⌘click` anywhere in the PDF to see
the source for that location, including the file, line number, and copyable text. This also works
for saved versions whose source is no longer on disk.

### Neovim

[press.nvim](https://github.com/antonio-leitao/press.nvim) is a thin wrapper around Press for
Neovim. Its `:Press` command sends the path of the current file to Press, which then finds the
document it belongs to. The plugin locates an installed copy of Press automatically, so it does
not require `--link` or any configuration.

The plugin only opens the document. Press itself handles compilation, file watching, and
reloading. An integration for another editor would only need to send the current file's path to
Press; without one, you can open the file through Press directly.

## Versions

Because Press treats a PDF as a view, it can create a view for any saved state of the source. Use
`⌘K` to save and title a snapshot of the current document. Snapshots are always deliberate, never
automatic. Your editor's undo history already records individual changes; Press keeps only the
versions you decide are worth returning to.

The sidebar pins the working copy at the top, followed by each saved version, its age, and its
build status. Select a version to read it just like the working copy. If Press has not compiled it
before, it does so in the background. Press builds versions from temporary copies and writes
nothing to your project folder.

This history is **not** a replacement for git. It has no branches, merges, or remotes. It is simply
a list of document versions stored outside the project, independent of any version control you
already use. Press deduplicates identical content, so if one hundred versions of a thesis use the
same figures, Press stores those figures only once.

Press stores only the files that belong to the document: its source, figures, `.bib` files, and
included chapters. It excludes build output, editor scratch files, `.git`, and unrelated documents
in the same folder.

## Markdown

A Markdown file works like any other document in Press, with the same viewer, file watching, and
history. Press runs the file through pandoc and then latexmk automatically.

Markdown does not provide a place for a LaTeX preamble, so Press lets you define one. Settings →
**Markdown frontmatter** contains named YAML presets for options such as fonts, `geometry`,
`documentclass`, and `header-includes`. You can apply a preset to any Markdown document. The
document's own frontmatter takes precedence for matching keys, so a document-specific title does
not discard the preset's typography. Values in `header-includes`, such as `\usepackage` commands,
are added to the preset rather than replacing it.

One limitation: Markdown build errors identify the file but not the source line. Line numbers in
the build log refer to the LaTeX generated by pandoc, not to the original Markdown, so Press does
not display a potentially misleading line number.

## Keys

The viewer uses [zathura](https://pwmt.org/projects/zathura/)'s keymap and supports numeric counts.
For example, `12G` goes to page 12, and `3j` scrolls down three steps.

| Key                 | Action                                          |
| ------------------- | ----------------------------------------------- |
| `j` / `k`           | scroll down / up                                |
| `h` / `l`           | scroll left / right                             |
| `d` / `u`           | scroll half a page down / up                    |
| `f` / `b` / `space` | move one page down / up                         |
| `J` / `K`           | go to the next / previous page                  |
| `gg` / `G`          | go to the first / last page                     |
| `12G`               | go to page 12                                   |
| `⌃o` / `⌃i`         | go back / forward after a jump                  |
| `+` / `-`           | zoom in / out                                   |
| `0`                 | use the actual page size                        |
| `a` / `s`           | fit the page / page width                       |
| `⌃r`                | toggle dark mode for the page and interface     |
| click               | follow a reference, citation, or link           |
| `⌘click`            | show the source for that location in the PDF    |
| `R`                 | rebuild the selected version                    |
| `⌘K`                | save a snapshot of the working copy             |
| `?`                 | show this key list in the app                    |

## Settings

- **Theme** — choose light mode, dark mode, or the system setting. Dark mode also inverts the page
  for nighttime reading.
- **Icon** — choose from three Dock icons.
- **Editor** — the command the Editor button runs.
- **Markdown frontmatter** — the presets described [above](#markdown), with a live preview.

From a document's library entry, you can rename it, change its TeX engine, or remove it. There is
no "change the main file" setting because Press treats each main document as a separate project.
This allows several projects to share one folder.

## Requirements

- macOS
- A TeX distribution with `latexmk` (MacTeX, TeX Live, BasicTeX)
- pandoc, for Markdown documents only
- Node.js and Rust, to build Press

Press does not enable `--shell-escape`. It does warn you when a document's folder contains a
`.latexmkrc`, which latexmk executes as Perl. Only open documents from folders you trust.

## Building from source

For most users, `npm run install:app` is all that is needed. To run individual build steps, use:

```sh
npm run tauri build            # build without installing
sh scripts/install-app.sh      # install without building
npm run tauri dev              # run it, rebuilt as you edit
npm run test                   # svelte-check, frontend tests, clippy, cargo test
```

The installed application and the development build are both named `press`, and only one can run
at a time. While developing Press, set `press.nvim`'s `app_command` to
`src-tauri/target/debug/press`.

## Not there yet

- Text selection and in-document search
- Side-by-side version comparison
- Configurable compiler arguments
- Export and print
- Linux and Windows

## License

Copyright (C) 2025 Antonio Leitao. GNU Affero General Public License, version 3 — the full text is
in [LICENSE](LICENSE).

Press uses MuPDF, which is licensed under AGPL-3.0, and compiles its C code into the application.
As a result, Press must be distributed under the same license.
