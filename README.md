# Press

Press is a Tauri desktop app that keeps LaTeX source trees clean while providing a cached PDF viewer and a Neovim workflow.

This repository currently contains the first backend-focused phase. The interface is intentionally plain.

## Included

- Add a project with the native folder picker.
- Bounded, recursive main-file discovery using `\\documentclass`, conventional filenames, folder depth, and `% !TEX root` evidence.
- A disambiguation picker when there is no safe main-file choice.
- TeX engine detection from `% !TEX program` (`pdflatex`, `xelatex`, or `lualatex`).
- Rust-owned SQLite persistence for recent projects and build state.
- One active watcher/build session at a time. Changes from any editor trigger builds.
- A single desktop application instance, preventing competing builds against one cache.
- Debounced, sequential `latexmk` builds in a warm managed cache outside the source tree.
- Last-good-PDF publishing: failed or superseded builds never replace the visible PDF.
- Persisted PDFs rendered with pdf.js, including first-page thumbnails in the project library.
- A **Launch Neovim** action that opens Neovim in Alacritty and reuses one stable server socket per project.
- Build-process-group termination on project switches and app exit.
- Startup removal of interrupted staging files and unreferenced PDF artifacts.

## Requirements

- macOS for the currently tested desktop integration
- A TeX distribution containing `latexmk` in `PATH` or a standard macOS location
- Neovim and Alacritty
- Node.js and Rust for development

Press deliberately does not enable `--shell-escape`. A project-local `.latexmkrc` is still executable configuration, so only add folders you trust.

## Development

```sh
npm install
npm run check
cargo test --manifest-path src-tauri/Cargo.toml
npm run tauri dev
```

## Backend boundaries

The Svelte webview can open the native folder picker and invoke a small set of typed application commands. It has no generic filesystem, shell, or SQL access. Rust owns path validation, discovery, persistence, process execution, cache publication, and lifecycle cleanup.

SQLite and successful PDF artifacts live in Tauri's application-data directory. Reusable LaTeX auxiliary files and the latest build log live in the application-cache directory. Only an explicit future export feature will write generated files into a source project.

## Deliberately deferred

- Neovim RPC diagnostics and quickfix synchronization
- Configurable editor, terminal, and compiler arguments
- Export/print
- Multi-window projects
- Semantic scroll anchoring across changed pagination
- Polished visual design
