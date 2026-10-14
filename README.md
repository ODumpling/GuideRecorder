# GuidesYa

Desktop step recorder. Record what you do, then edit the guide and export it to PDF, DOCX, Markdown or HTML. Local-first, with AI-written step text and a read-only MCP server.

Design docs live in Solo project 51 scratchpads (overview: #437).

## Layout

```
crates/
  guidesya-core/      format, library, index, step detection, export, AI
  guidesya-capture/   CaptureBackend trait + macOS backend
  guidesya-mcp/       stdio MCP server (binary: guidesya-mcp)
  guidesya-cli/       CLI (binary: guidesya)
app/                  Tauri 2 app: React/Vite/TS/Tailwind/shadcn
  src-tauri/          guidesya-app crate
fixtures/guides/      fixture guides for tests
```

## Develop

```sh
cd app && pnpm install
pnpm tauri dev          # run the app (Vite on :1430)
cargo test --workspace  # from repo root
```

macOS permissions (Screen Recording, Accessibility) only survive rebuilds when
builds are signed with a stable identity. Run `scripts/macos-signing.sh create`
once; see [docs/signing.md](docs/signing.md).

## Use with Claude Code (MCP)

```sh
cargo build --release -p guidesya-mcp
claude mcp add guidesya -- "$PWD/target/release/guidesya-mcp" --library ~/Documents/GuidesYa
```

The server is read-only. See [crates/guidesya-mcp/README.md](crates/guidesya-mcp/README.md).

## CLI

```sh
cargo run -p guidesya-cli -- --library <dir> list | search <q> | validate <bundle>
cargo run -p guidesya-cli -- --library <dir> --index <path> reindex   # the only writer
```
