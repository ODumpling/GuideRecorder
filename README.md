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
