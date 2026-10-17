# thelab

The command-line interface and MCP server of The Lab, built on `thelab-core`.
Claude Code runs it as the user-scope MCP server `thelab`:

```bash
cargo install --path crates/thelab
claude mcp add --transport stdio --scope user thelab -- thelab mcp
```

Its stdout carries only JSON-RPC messages; all logging goes to stderr.

Origin: this crate came from github.com/lukejwaszyn/the-lab-native, Luke
Waszyn's MIT-licensed code (see `../LICENSE`). It was brought into The Lab in
milestone F3, merged from that repository's `claude/python-terminal` (8f7929d)
and `claude/biber-missing` (ab11a6b) branches.
