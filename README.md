<p align="center">
  <img src='static/press.svg' height='200px' align="center"></img>
</p>

<div align="center">
<h3 max-width='200px' align="center">Printing Press</h3>
  <p><i>A PDF viewer that does not need a PDF<br/>
  Hand it LaTeX or Markdown — compiling is the viewer's job, not yours<br/>
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

Press is a PDF viewer that does not need a PDF. You give it LaTeX or Markdown source, and it shows
you the document.

Working on a document normally comes in three parts, and you own all three: write the source,
compile it, open the result. Press moves the parentheses:

`(write + compile) + view` → `write + (compile + view)`

Compiling stops being a step in your work and becomes part of the viewer. You are left with the
one part that was ever yours — the source. Producing a document from it is the viewer's problem,
the way rendering glyphs is a viewer's problem, and you never ask for it.

Everything else in Press follows from that regrouping:

- **The viewer watches your source itself.** Because the compile happens inside Press, Press knows
  when to run it. Save in your editor and what you are looking at is already current. There is no
  `latexmk -pvc`, no watcher script, no viewer to tell to reload, no three programs to wire
  together — the loop is closed inside one application, and there is nothing about it to set up.
- **The PDF is a view, not a product.** It is not a file you made and now have to keep, name, or
  clean up. Press stores it in its own cache and nothing is ever written into your project folder.
  Delete Press tomorrow and your folder is exactly the source you wrote.
- **A view is cheap, so any state of the source can have one.** That is what the history is: `⌘K`
  keeps the document as it stands right now, under a title, and any version you kept can be
  looked at like the live one — Press compiles it if it never has. No version is the final one.
  The document is the source; a PDF is only what it looks like at some moment.

## Installation

Press is not distributed as a download — you build it and install it yourself. From a fresh clone:

```sh
npm install
npm run install:app
```

That puts `Press.app` in `/Applications`. It is safe to run again to upgrade. The first build is
slow, because the PDF renderer is C that gets compiled in; later ones are not.

To also get a `press` command in your terminal:

```sh
npm run install:app -- --link
```

That writes a small script to `~/.local/bin/press`, so you can run `press paper.tex` from
anywhere. You only need the flag once.

To remove Press:

```sh
npm run uninstall:app            # the app
npm run uninstall:app -- --purge # the app and every version it stored
```

Without `--purge` your history stays. Snapshots live in Press's own storage rather than in your
project folders, so that folder is the only copy of the versions you kept — rebuilding the app is
easy, and that is not.

## Opening a document

Three ways in, all equivalent:

- **Drag** a `.tex` or `.md` file onto the Press window, or use the Add button.
- **`press paper.tex`** from the terminal, if you linked the command.
- **`:Press`** from Neovim.

Point Press at a file and it opens that document. Point it at a folder and it lists the documents
it found — the ones you already use first — and lets you pick. It never chooses for you.

Naming any file opens the document it belongs to. `:Press` inside `chapters/three.tex` opens the
thesis, because Press follows `% !TEX root` and the `\input` graph up to the real document root.
Two papers in one folder are two separate projects, each with its own history and its own build.

You do this once per document. From then on it is in the library, and opening it is opening a
view — there is nothing to rebuild first, and no PDF to go and find.

## Editing

Press does not edit anything. The **Editor** button hands the file to whatever command you set in
Settings — `{file}` is the document, `{dir}` its folder. Leave it unset and the file opens in
whatever your system already uses for it.

Saving rebuilds the document, whatever wrote the file — Vim, VS Code, Emacs, TextEdit, a script
running behind your back. Press watches the files itself and needs nothing from the editor's side:
no plugin, no save hook, no reload command, no port to agree on. The PDF updates in place, keeps
your scroll position, and never blanks while it recompiles.

Click a reference, a citation or a link in the PDF to jump to it. `⌘click` anywhere in the PDF
shows the source behind that spot — the file, the line, and the text itself, which you can copy.
It works on old versions too, whose source is no longer on disk.

### Neovim

[press.nvim](https://github.com/antonio-leitao/press.nvim) is a thin wrapper around Press for
Neovim users. It adds `:Press`, which sends the path of the file you are editing and lets Press
work out which document that is. It finds an installed Press on its own, so it needs no `--link`
and no configuration.

That is the whole plugin. It opens a document and then stops — it takes no part in compiling,
watching or reloading, because none of those need anything from your editor. A wrapper for any
other editor would be the same one line, and not having one costs you only the keystroke.

## Versions

Once the PDF is a view rather than a product, there is no reason only the current source should
have one. `⌘K` saves the document as it stands, under a title. It is deliberate and never
automatic: your editor's undo already covers keystrokes, and a history is worth reading only when
every entry was meant.

The sidebar lists your working copy pinned at the top, then each saved version with its age and
whether it builds. Select one and you are reading it, exactly as you read the live document —
Press compiles it in the background if it never has, and you find out only because the page
arrives. Versions are compiled from a temporary copy, so nothing is ever written into your folder.

This is **not** git, and it does not want to be. There are no branches, no merges, no remotes —
just a list of versions of one document, kept outside the project and independent of whatever
version control you already use. Identical content is stored once, so a hundred versions of a
thesis whose figures never change store those figures once.

Only this document's files are stored: its source, its figures, its `.bib`, the chapters it
includes. Build output, editor scratch files, your `.git`, and the other documents sharing the
folder are all left out.

## Markdown

A Markdown file is a document like any other — same viewer, same watching, same history. Press
runs it through pandoc and then latexmk, which is a thing you are being told rather than a thing
you have to do.

Markdown has nowhere to put a LaTeX preamble, so Press lets you keep one. Settings → **Markdown
frontmatter** holds named presets: a block of YAML — fonts, `geometry`, `documentclass`,
`header-includes` — applied to every markdown document. A document's own frontmatter wins key by
key, so setting a title in one file keeps the preset's typography, and adding a `\usepackage` adds
to the preset's preamble rather than replacing it.

One limitation worth knowing: errors in a markdown build name the file but carry no line number.
The numbers in the log refer to the LaTeX pandoc generated, and pointing at a line in your
markdown would be plausible and wrong.

## Keys

The viewer uses [zathura](https://pwmt.org/projects/zathura/)'s keymap, counts included — `12G`
goes to page 12, `3j` scrolls three steps.

| Key                 | Does                                         |
| ------------------- | -------------------------------------------- |
| `j` / `k`           | scroll down / up                             |
| `h` / `l`           | scroll left / right                          |
| `d` / `u`           | half page down / up                          |
| `f` / `b` / `space` | page down / up                               |
| `J` / `K`           | next / previous page                         |
| `gg` / `G`          | first / last page                            |
| `12G`               | go to page 12                                |
| `⌃o` / `⌃i`         | back / forward after a jump                  |
| `+` / `-`           | zoom in / out                                |
| `0`                 | actual size                                  |
| `a` / `s`           | fit page / fit width                         |
| `⌃r`                | dark theme — the page, and Press around it   |
| click               | follow a reference, a citation or a link     |
| `⌘click`            | show the source behind that place in the PDF |
| `R`                 | rebuild this version                         |
| `⌘K`                | snapshot the working copy                    |
| `?`                 | this list, in the app                        |

## Settings

- **Theme** — light, dark, or follow the system. Dark inverts the page too, for reading at night.
- **Icon** — three Dock icons to choose from.
- **Editor** — the command the Editor button runs.
- **Markdown frontmatter** — the presets described [above](#markdown), with a live preview.

Per document, from its entry in the library: rename it, change its TeX engine, or remove it.
There is no "change the main file" — a different document is a different project, which is what
lets several live in one folder.

## Requirements

- macOS
- A TeX distribution with `latexmk` (MacTeX, TeX Live, BasicTeX)
- pandoc, for Markdown documents only
- Node.js and Rust, to build Press

Press does not enable `--shell-escape`. It does warn you when a document's folder contains a
`.latexmkrc`, which latexmk would execute as Perl — open documents in folders you trust.

## Building from source

`npm run install:app` covers the normal case. The pieces, when you want only one of them:

```sh
npm run tauri build            # build without installing
sh scripts/install-app.sh      # install without building
npm run tauri dev              # run it, rebuilt as you edit
npm run test                   # svelte-check, frontend tests, clippy, cargo test
```

An installed Press and a development build are both called `press`, and only one runs at a time.
Point `press.nvim`'s `app_command` at `src-tauri/target/debug/press` while working on Press
itself.

## Not there yet

- Text selection and in-document search
- Side-by-side version comparison
- Configurable compiler arguments
- Export and print
- Linux and Windows

## License

Copyright (C) 2025 Antonio Leitao. GNU Affero General Public License, version 3 — the full text is
in [LICENSE](LICENSE).

Not a choice so much as an inheritance: Press renders with MuPDF, which is AGPL-3.0, and its C is
compiled into the binary. Anything linking it goes out under the same terms.
