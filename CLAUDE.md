# The Lab: guidance for Claude Code

The Lab is a fork of ClaudePrism (`upstream`: delibae/claude-prism): a Tauri 2 desktop app with a Rust backend in `apps/desktop/src-tauri` and a React frontend in `apps/desktop/src`. The fork's plan, milestones F1 to F4, is in Luke's plan file `the-lab-fork.md` and in this repository's commits.

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
PKG_CONFIG_PATH=/opt/homebrew/opt/icu4c@78/lib/pkgconfig \
CXXFLAGS="-std=c++17 -I/opt/homebrew/include" CFLAGS="-I/opt/homebrew/include" \
CARGO_PROFILE_DEV_DEBUG=0 \
npx -y pnpm@10.28.2 --filter=@claude-prism/desktop tauri build --debug --bundles app
```

- **corepack:** Node 26 ships without it, and `tauri.conf.json` runs `corepack pnpm`. Use a stand-in script named `corepack` that runs `npx -y pnpm@10.28.2 "$@"` after dropping its first argument (`pnpm`).
- **The embedded Tectonic** builds against Homebrew's keg-only icu4c 78, which needs C++17, and includes `<harfbuzz/hb.h>`, which needs `-I/opt/homebrew/include`.
- **Debug size:** with full debug info, `target/debug/deps` reached 3.5 GB on a nearly full disk. `CARGO_PROFILE_DEV_DEBUG=0` keeps it small. Check `df -h ~` before building.
- **The app** is then at `apps/desktop/src-tauri/target/debug/bundle/macos/The Lab.app`.

## Checks
- `npx -y pnpm@10.28.2 --filter=@claude-prism/desktop test` (vitest) and `npx -y pnpm@10.28.2 lint` (biome)
- `cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml`
- `cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml live_permission_probe -- --ignored --nocapture` runs the real `claude` once with haiku to check the permission protocol end to end. It spends tokens: run it only when that code or the CLI version changes, with `CLAUDECODE` unset.
- Verify behaviour in the running app; Luke does the visual QA.
