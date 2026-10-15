# GuideRecorder

Desktop step recorder. Record what you do, then edit the guide and export it to PDF, DOCX, Markdown or HTML. Local-first, with AI-written step text and a read-only MCP server.

Design docs live in Solo project 51 scratchpads (overview: #437).

## Layout

```
crates/
  guiderecorder-core/      format, library, index, step detection, export, AI
  guiderecorder-capture/   CaptureBackend trait + macOS backend
  guiderecorder-mcp/       stdio MCP server (binary: guiderecorder-mcp)
  guiderecorder-cli/       CLI (binary: guiderecorder)
app/                  Tauri 2 app: React/Vite/TS/Tailwind/shadcn
  src-tauri/          guiderecorder-app crate
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
cargo build --release -p guiderecorder-mcp
claude mcp add guiderecorder -- "$PWD/target/release/guiderecorder-mcp" --library ~/Documents/GuideRecorder
```

The server is read-only. See [crates/guiderecorder-mcp/README.md](crates/guiderecorder-mcp/README.md).

## CLI

```sh
cargo run -p guiderecorder-cli -- --library <dir> list | search <q> | validate <bundle>
cargo run -p guiderecorder-cli -- --library <dir> --index <path> reindex   # the only writer
```

## Licence

GuideRecorder is released under the [MIT License](LICENSE). Third-party licences are listed in [docs/licences.md](docs/licences.md).
