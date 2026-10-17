# thelab-core

The Lab's LaTeX core: project scanning, the outline, log parsing, compile
driving (Tectonic, or latexmk for biblatex documents that need biber, with
biber's own errors and biber runs that take turns across processes), SyncTeX,
the bibliography, page rendering and the Python interpreter per folder.

The desktop app (`apps/desktop/src-tauri`) compiles through
`thelab_core::compile::compile`; the `thelab` crate next to this one is the
CLI and MCP server built on it.

Origin: this crate came from github.com/lukejwaszyn/the-lab-native, Luke
Waszyn's MIT-licensed code (see `../LICENSE`). It was brought into The Lab in
milestone F3, merged from that repository's `claude/python-terminal` (8f7929d)
and `claude/biber-missing` (ab11a6b) branches.

Tests: `cargo test --manifest-path crates/Cargo.toml`. The compile tests run
the real engines when they are installed and skip what is missing.
