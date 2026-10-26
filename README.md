<p align="center">
  <img src="static/press.svg" height="200" alt="Press" />
</p>

<div align="center">
  <h3>Printing Press</h3>
  <p><i>Open a LaTeX or Markdown source file. Press compiles and displays it.<br />
  Built with Rust.</i></p>
  <p>
    <img alt="macOS" src="https://img.shields.io/badge/macOS-black?style=for-the-badge&logo=apple&logoColor=white">
    <img alt="Rust" src="https://img.shields.io/badge/rust-black?style=for-the-badge&logo=rust&logoColor=white">
    <img alt="AGPL-3.0" src="https://img.shields.io/badge/AGPL--3.0-black?style=for-the-badge">
  </p>
</div>

<p align="center">
  <img src="static/final_final.pdf.png" width="420" alt="A llama surrounded by potion bottles labelled with variations of final.pdf" />
</p>

#

### Contents

- [Installation](#installation)
- [Opening a document](#opening-a-document)
- [Starting a document](#starting-a-document)
- [Editing](#editing)
  - [Neovim](#neovim)
- [Versions](#versions)
- [Markdown](#markdown)
- [Keys](#keys)
- [Settings](#settings)
- [Requirements](#requirements)
- [License](#license)

Press is a macOS viewer for LaTeX and Markdown source files. Open a source file and Press compiles
it, displays the result, and rebuilds it whenever you save.

- **Automatic rebuilds.** Press watches the source and updates the view after each save. You do not
  need `latexmk -pvc`, a watcher script, or a reload command.
- **No generated files in the project.** Press keeps compiled PDFs and auxiliary files in its own
  cache. [Creating a document](#starting-a-document) is the only operation that writes source
  files to your project folder.
- **Titled snapshots.** Press can store selected versions of the source and compile any of them for
  viewing. Press history is separate from Git and your editor's undo history.

## Installation

Press is not distributed as a prebuilt download. After cloning the repository, build and install
it with:

```sh
npm install
npm run install:app
```

These commands install `Press.app` in `/Applications`. Run them again to upgrade. The first build
takes longer because it compiles the PDF renderer's C code.

To add a `press` command to your terminal, run:

```sh
npm run install:app -- --link
```

This writes a small script to `~/.local/bin/press`, so you can run `press paper.tex` from any
directory. Use the flag once.

To remove Press:

```sh
npm run uninstall:app            # the app
npm run uninstall:app -- --purge # the app and every version it stored
```

Without `--purge`, Press keeps your document history. Its storage contains the only copy of those
snapshots, so purge it only when you no longer need them.

## Opening a document

You can open a document in any of three ways:

- **Drag** a `.tex` or `.md` file onto the Press window, or use the Add button.
- **`press paper.tex`** from the terminal, if you linked the command.
- **`:Press`** from Neovim.

Give Press a file and it opens the corresponding document. Give it a folder and it lists the
documents it finds, with recently used documents first.

You can open a document from any file that belongs to it. For example, running `:Press` while
editing `content/chapter_one.tex` opens the full book, even when `main.tex` only includes
`content/index.tex` and that file includes the chapters. Press follows `% !TEX root` directives,
the main file a `subfiles` chapter names, and the `\input` graph however deep it goes, reading
paths the way TeX does. A file with no `\documentclass` that nothing includes cannot compile on
its own, so Press opens the document above it instead: `main.tex` if there is one. If a folder contains two papers, Press treats them as
separate projects, with separate builds and histories.

You only need to add a document once. Press keeps it in the library after that.

The top bar is a row of icons and one button. From the left: filter, grouping, settings, new
document, and **Open document**.

The library can group documents by the folders they are in. The grouping icon shows how deep the
grouping goes, one indent per level. Click it to open a small slider with the levels and how many
groups each one makes. The first step makes a few broad groups, and each step after splits them by
the next folder down. A folder that only leads to one other folder counts as one step, and a folder
holding a single document doesn't become a group. Click a group's name to fold it.

Click the filter icon (or press `/` or `⌘F`) to open the filter, then type to narrow the library to
documents whose name or path contains every word you type. `Enter` opens the document when only
one is left, and `Esc` clears the filter and closes it.

## Starting a document

**New document** can start with a blank Markdown or LaTeX file, or with one of your templates. Name
the document, select a folder, and Press creates the files there and opens it.

A blank Markdown document is an empty `.md` file. A blank LaTeX document is a minimal `article`
with one blank page. Both create one named file directly in the selected folder.

Press ships without templates. To create one, right-click a document in the library and choose
**Save as template**. You can save only the main file or select files from the document tree. Press
excludes build output, editor scratch files, `.git`, and unrelated documents. The main file is
always included.

A template is a copy. Later changes to the original project do not affect it, and Press does not
alter the source while saving or restoring it. Press also stores the last compiled page for the
gallery card and remembers the TeX engine.

A single-file template creates one file in the selected folder. A multi-file template creates a
folder named after the new document. Press refuses to overwrite an existing file or folder.

Document creation is the only feature that writes source files. Builds, snapshots, and auxiliary
files stay in Press's storage.

## Editing

Press is a viewer, not an editor. The **Editor** button runs the command you choose in Settings.
Use `{file}` as a placeholder for the document and `{dir}` for its folder. If you leave the command
unset, Press opens the file with the system's default application.

Click the location of a build error to open the editor at it. The same command runs, with `{file}`
as the file the error is in and `{line}` as its line, so write the line where your editor wants
it: `nvim +{line} {file}`, `code --goto {file}:{line}`. Opened from the button, `{line}` is 1.

Press watches files directly, so it works with Vim, VS Code, Emacs, TextEdit, and background
scripts without a plugin or save hook. After a change, it rebuilds the document and updates the
page in place while preserving your scroll position.

A save during a build lets that build finish, then builds again. `R` restarts a build that is
already running. Press stops a build that prints nothing for 2 minutes, which is what TeX does
when a macro expands forever, and any build that runs for 30 minutes.

Click a reference, citation, or link in the PDF to follow it. `⌘click` anywhere in the PDF to see
the source for that location, including the file, line number, and copyable text. This also works
for saved versions whose source is no longer on disk.

### Neovim

[press.nvim](https://github.com/antonio-leitao/press.nvim) adds a `:Press` command to Neovim. It
sends the current file path to Press, which finds and opens the document root. The plugin locates
the installed app automatically and does not require `--link` or configuration.

`:Press` also sends the line your cursor is on. Press asks SyncTeX where that line ended up,
scrolls the PDF there and marks the line for a moment. Run it again after moving to jump again; a
document that is already open is not reopened or rebuilt. From a terminal, the same thing is
`press chapters/two.tex --line=42`.

The plugin only opens documents. Press handles compilation, file watching, and reloading.

## Versions

Use `⌘K` to save and title a snapshot of the current document. Press never creates snapshots
automatically.

The sidebar pins the working copy at the top, followed by each saved version, its age, and its
build status. Select a version to read it just like the working copy. If Press has not compiled it
before, it does so in the background. Press builds versions from temporary copies and writes
nothing to your project folder.

Each version shows how many words it has, counted from the source with texcount (LaTeX) or
directly (Markdown), and the working tree shows how many it has gained since the newest snapshot.
The top of the history draws the working tree's count over the last month, a point per day.

Git still handles branches, merges, and remotes. Press history is a flat list of document versions
stored outside the project. Press deduplicates identical content, so snapshots that use the same
figures store one copy of each figure.

Press stores only the files that belong to the document: its source, figures, `.bib` files, and
included chapters. It excludes build output, editor scratch files, `.git`, and unrelated documents
in the same folder.

## Markdown

A Markdown file uses the same viewer, file watching, and history as a LaTeX file. Press runs it
through pandoc and then latexmk.

Set `geometry`, `documentclass`, `fontsize`, and `header-includes` in the document's YAML
frontmatter. Press passes those values to pandoc without adding its own formatting.

One limitation: Markdown build errors identify the file but not the source line. Line numbers in
the build log refer to the LaTeX generated by pandoc, not to the original Markdown, so Press does
not display a potentially misleading line number.

## Keys

The viewer uses [zathura](https://pwmt.org/projects/zathura/)'s keymap and supports numeric counts.
For example, `12G` goes to page 12, and `3j` scrolls down three steps.

| Key                 | Action                                       |
| ------------------- | -------------------------------------------- |
| `j` / `k`           | scroll down / up                             |
| `h` / `l`           | scroll left / right                          |
| `d` / `u`           | scroll half a page down / up                 |
| `f` / `b` / `space` | move one page down / up                      |
| `J` / `K`           | go to the next / previous page               |
| `gg` / `G`          | go to the first / last page                  |
| `12G`               | go to page 12                                |
| `⌃o` / `⌃i`         | go back / forward after a jump               |
| `+` / `-`           | zoom in / out                                |
| `0`                 | use the actual page size                     |
| `a` / `s`           | fit the page / page width                    |
| `/`                 | search the document                          |
| `n` / `N`           | go to the next / previous match              |
| `⌃r`                | toggle dark mode for the page and interface  |
| click               | follow a reference, citation, or link        |
| `⌘click`            | show the source for that location in the PDF |
| `R`                 | rebuild the selected version, restarting it  |
| `⌘K`                | save a snapshot of the working copy          |
| `?`                 | show this key list in the app                |

## Requirements

- macOS
- A TeX distribution with `latexmk` (MacTeX, TeX Live, BasicTeX)
- pandoc, for Markdown documents only
- Node.js and Rust, to build Press

Press does not enable `--shell-escape`. It does warn you when a document's folder contains a
`.latexmkrc`, which latexmk executes as Perl. Only open documents from folders you trust.

## License

Copyright (C) 2026 Antonio Leitao. GNU Affero General Public License, version 3.
The full text is in
[LICENSE](LICENSE).

Press uses MuPDF, which is licensed under AGPL-3.0, and compiles its C code into the application.
As a result, Press must be distributed under the same license.
