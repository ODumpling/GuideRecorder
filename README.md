# GuideRecorder

**Do a task once, get a step-by-step guide.**

GuideRecorder watches what you do on your Mac and turns it into a how-to guide:
one step per click or shortcut, each with a screenshot of the moment you did
it. You then tidy the guide up in a built-in editor and export it as PDF, Word,
Markdown or HTML to share with your team, customers or future self.

Everything stays on your machine. AI writing is optional and uses your own
Anthropic API key, any OpenAI-compatible API, or a local model in Ollama or
LM Studio, in which case nothing leaves your Mac.

> **Status:** early and in active development. macOS only for now (Windows is
> planned). Download the latest version from
> [Releases](https://github.com/ODumpling/GuideRecorder/releases) (it updates
> itself), or build it from source (see [Build from source](#build-from-source)).

## How it works

1. **Record.** Press Record (or the record shortcut, ⌃⌥R by default) and do the task as you normally would.
   GuideRecorder notes each click, typed text and shortcut, and takes a
   screenshot of each one. A small floating toolbar shows the elapsed time and
   the number of screenshots, with Pause, Stop and a button to add a manual
   step. You can record your whole screen, a single window or a region you
   draw.
2. **Edit.** When you stop, the guide opens in the editor. Each step has a
   title and description (written for you from what you clicked, e.g. "Click
   **Save**"), and you can:
   - reword, reorder, merge, split or delete steps, and add sections and notes;
   - annotate screenshots with arrows, boxes, highlights, callouts and blurs,
     and switch between a close-up crop and the full screenshot;
   - undo and redo everything; changes save automatically.
3. **Let AI write it (optional).** With an API key set, **Write with AI** turns
   the raw steps into clear instructions and writes a title, intro,
   prerequisites, summary and tags. It never overwrites text you have edited
   yourself. Per step you can also **Regenerate** or **Rewrite** it shorter, for
   beginners, for experts, or with your own instruction. The AI can also spot
   things worth hiding (emails, account numbers) and suggest blurs, which you
   accept or reject.
4. **Export.** Save the guide as **PDF**, **Word (DOCX)**, **Markdown** or
   **HTML**, with your logo if you like. PDF and Word come on A4 or Letter
   paper (the default follows your region). Blurs are applied to the actual pixels
   on export, so hidden information is really gone.

## Annotation guides

Not everything is a sequence of clicks. To explain a floor plan, a form, a
dashboard or a brochure, create an **annotation guide** instead: choose
**New annotation guide…** in the library and pick one or more images
(PNG, JPEG, WebP) or a PDF. Each image or PDF page becomes a page of the guide.
On each page, draw **hotspots** around the parts worth explaining and give each
one a title and a description.

In an HTML export, readers hover over a hotspot, or click or tap it, to see its
title and description. The page needs no script, so it works anywhere a static
file opens. PDF, Word and Markdown exports show numbered markers on the image
with a numbered list of the hotspots underneath. Hotspots work on recorded
steps too.

## Privacy

GuideRecorder is built to be safe to leave running:

- **Local-first.** Guides are ordinary folders on your disk (one
  `.guiderecorder` folder per guide, holding a `guide.json` and its images),
  so they work with any backup or sync tool. Nothing is uploaded.
- **Password fields are never recorded.** Keystrokes in secure fields are
  masked before they are stored.
- **Excluded apps.** While an app on your exclusion list is in front, nothing
  from it is captured: no screenshots, keys or clicks. Its windows are also
  kept out of every screenshot, even behind other apps. By default this covers
  1Password, Bitwarden, Keychain Access, Passwords and Messages, and you can
  edit the list in Settings.
- **AI only when you say so.** Nothing is sent anywhere unless AI is turned
  on and you have added your own key, which is kept in the macOS Keychain.
  Blur suggestions are only suggestions until you accept them.

## Requirements

- macOS 12.3 or later.
- Permissions, granted on first launch: **Screen Recording** (screenshots),
  **Accessibility** (which button you clicked) and **Input Monitoring**
  (clicks and keys).
- Optional, for AI writing: an [Anthropic API key](https://console.anthropic.com/),
  an OpenAI-compatible API, or [Ollama](https://ollama.com/) or
  [LM Studio](https://lmstudio.ai/) with a model that supports tool calls.

## Keyboard shortcuts

The defaults; change them in Settings → Recording.

| Shortcut | Action |
| --- | --- |
| ⌃⌥R | Start or stop recording |
| ⌃⌥P | Pause or resume |
| ⌃⌥S | Add a manual step while recording |

## Use your guides from Claude (MCP)

GuideRecorder includes a read-only [MCP](https://modelcontextprotocol.io)
server, so Claude and other MCP clients can list, search and read your guides,
for example "how do we rotate the API key?".

```sh
cargo build --release -p guiderecorder-mcp
claude mcp add guiderecorder -- "$PWD/target/release/guiderecorder-mcp" --library ~/Documents/GuideRecorder
```

See [crates/guiderecorder-mcp/README.md](crates/guiderecorder-mcp/README.md).

## Command line

A small CLI lists, searches, validates and exports guides, and rebuilds the
search index:

```sh
cargo run -p guiderecorder-cli -- --library ~/Documents/GuideRecorder list
cargo run -p guiderecorder-cli -- --library ~/Documents/GuideRecorder search "api key"
cargo run -p guiderecorder-cli -- --library ~/Documents/GuideRecorder validate "<guide>.guiderecorder"
cargo run -p guiderecorder-cli -- --library ~/Documents/GuideRecorder --index <path> reindex
cargo run -p guiderecorder-cli -- --library ~/Documents/GuideRecorder export "<guide>.guiderecorder" --format pdf --out guide.pdf --paper letter
```

`export` takes `--format md|html|walkthrough|docx|pdf` (`walkthrough` is an
interactive click-through `guide.html`), `--out`, and optionally
`--paper a4|letter` (DOCX and PDF; default A4), `--logo`, `--accent`,
`--overwrite` and `--strict`. See `guiderecorder export --help`.

## Build from source

You need [Rust](https://rustup.rs), [Node.js](https://nodejs.org) and
[pnpm](https://pnpm.io).

```sh
git clone https://github.com/ODumpling/GuideRecorder.git
cd GuideRecorder/app
pnpm install
pnpm tauri dev
```

macOS ties permissions to the app's signature, so unsigned development builds
lose them on every rebuild. To avoid re-granting them each time, create a local
signing identity once with `scripts/macos-signing.sh create` (see
[docs/signing.md](docs/signing.md)).

### Project layout

```
crates/
  guiderecorder-core/      guide format, library, search index, step detection, export, AI
  guiderecorder-capture/   screen and input capture (macOS backend)
  guiderecorder-mcp/       read-only MCP server (binary: guiderecorder-mcp)
  guiderecorder-cli/       command line tool (binary: guiderecorder)
app/                       desktop app: Tauri 2, React, TypeScript, Tailwind
  src-tauri/               the app's Rust side (guiderecorder-app)
fixtures/guides/           sample guides used by the tests
docs/                      signing, releasing, capture smoke test, dependency licences
```

### Tests

```sh
cargo test --workspace                 # Rust, from the repo root
cd app && pnpm test && pnpm e2e        # frontend unit and end-to-end tests
```

CI runs these on every pull request. To publish a release, see
[docs/releasing.md](docs/releasing.md).

## Licence

GuideRecorder is released under the [MIT License](LICENSE). Third-party
licences are listed in [docs/licences.md](docs/licences.md).
