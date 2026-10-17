# The Lab: guidance for Claude Code

The Lab is a fork of ClaudePrism (`upstream`: delibae/claude-prism): a Tauri 2 desktop app with a Rust backend in `apps/desktop/src-tauri` and a React frontend in `apps/desktop/src`. The fork's plan, milestones F1 to F4, is in Luke's plan file `the-lab-fork.md` and in this repository's commits.

The Lab's own Rust crates are in `crates/` (their own Cargo workspace, `crates/Cargo.toml`, from github.com/lukejwaszyn/the-lab-native, MIT): `thelab-core` (scanning, log parsing, compile driving, SyncTeX, the bibliography, Python per folder) and `thelab` (the CLI and the MCP server that Claude Code runs as the user-scope server `thelab`). The repository root has no Cargo workspace, so src-tauri keeps its own and takes `thelab-core` by path.

## Rules
- **Claude checks; it does not ghostwrite.** In the app, Claude asks before it acts, reviews and answers, and edits only when the user asks. Keep defaults that way.
- **Permission prompts:** the chat runs `claude -p --input-format stream-json --permission-mode default --permission-prompt-tool stdio`, never `--dangerously-skip-permissions`. `claude_control.rs` builds and reads the control lines; `claude_process.rs` keeps each tab's stdin open until the run's `result`; the chat's cards are `permission-store.ts` and `permission-request-card.tsx`.
- **Stay mergeable with upstream.** Change visible branding to The Lab; leave internal names alone (`@claude-prism/*` packages, crate and module names, storage paths). `git fetch upstream` and merge their fixes; pushing to `upstream` is disabled on purpose.
- **Never push or publish without Luke's word.** The repository is public.
- **Auto-update is off.** The updater has no endpoints and creates no artifacts, until The Lab has its own signing key and releases. ClaudePrism's endpoint would replace The Lab with ClaudePrism.
- **License:** the code is MIT, but the app bundles MuPDF (AGPL), so the app as distributed is AGPL.

## Build on macOS
The traps found on 16 Oct 2026, on Node 26 with no pnpm installed:

```bash
npx -y pnpm@10.28.2 install
```

```bash
PATH="/path/to/a/folder/with/a/corepack/stand-in:$PATH" \
CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0 \
npx -y pnpm@10.28.2 --filter=@claude-prism/desktop tauri build --debug --bundles app
```

- **corepack:** Node 26 ships without it, and `tauri.conf.json` runs `corepack pnpm`. Use a stand-in script named `corepack` that runs `npx -y pnpm@10.28.2 "$@"` after dropping its first argument (`pnpm`).
- **No embedded Tectonic** (since F3): `latex.rs` compiles through `thelab_core::compile::compile`, which runs the installed `tectonic` or TeX Live's `latexmk`, chosen per document, so the ICU and HarfBuzz flags F1 needed (`PKG_CONFIG_PATH`, `CXXFLAGS`, `CFLAGS`) are gone. Keep whichever environment the target was built with, or the C crates rebuild.
- **One target folder:** build the crates workspace into src-tauri's target (`CARGO_TARGET_DIR=apps/desktop/src-tauri/target`); a second target does not fit on this disk.
- **Debug size:** with full debug info, `target/debug/deps` reached 3.5 GB on a nearly full disk. `CARGO_PROFILE_DEV_DEBUG=0` keeps it small. Check `df -h ~` before building.
- **The app** is then at `apps/desktop/src-tauri/target/debug/bundle/macos/The Lab.app`.

## Checks
- `npx -y pnpm@10.28.2 --filter=@claude-prism/desktop test` (vitest) and `npx -y pnpm@10.28.2 lint` (biome)
- `cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml`
- `CARGO_TARGET_DIR=apps/desktop/src-tauri/target cargo test --manifest-path crates/Cargo.toml` (the crates' tests; the compile tests run the real engines when installed)
- `THELAB_COMPILE_PROJECT=<a copy of a project> cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml compile_a_named_project -- --ignored --nocapture` compiles a project exactly as the app does and prints the report (it writes `.prism/` into the folder, so use a copy). `THELAB_EXPECT_BIB_ERROR=1` expects an error located in a `.bib` file.
- After changing `crates/`, reinstall the MCP server: `cargo install --path crates/thelab`, then `claude mcp get thelab`.
- `cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml live_permission_probe -- --ignored --nocapture` runs the real `claude` once with haiku to check the permission protocol end to end. It spends tokens: run it only when that code or the CLI version changes, with `CLAUDECODE` unset.
- Verify behaviour in the running app; Luke does the visual QA.
