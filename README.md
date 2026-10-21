# GuideRecorder

**Do a task once, get a step-by-step guide.**

GuideRecorder watches what you do on your Mac and turns it into a how-to guide:
one step per click or shortcut, each with a screenshot of the moment you did
it. You then tidy the guide up in a built-in editor and export it as PDF, Word,
Markdown, HTML, slides, a video or a GIF to share with your team, customers or future self.

Everything stays on your machine. AI writing is optional and uses your own
Anthropic API key, any OpenAI-compatible API, or a local model in Ollama or
LM Studio, in which case nothing leaves your Mac.

> **Status:** early and in active development, on macOS and, newer,
> Windows ([#14](https://github.com/ODumpling/GuideRecorder/issues/14)).
> On Windows the installer is not code-signed yet, so SmartScreen asks
> before the first run, and windows of excluded apps can still show in
> screenshots taken while another app is in front. Download the latest version from
> [Releases](https://github.com/ODumpling/GuideRecorder/releases) (it updates
> itself), or build it from source (see [Build from source](#build-from-source)).

## How it works

1. **Record.** Press Record (or the record shortcut, ⌃⌥R by default) and do the task as you normally would.
   GuideRecorder notes each click, typed text and shortcut, and takes a
   screenshot of each one. A small floating toolbar shows the elapsed time and
   the number of screenshots, with Pause, Stop and a button to add a manual
   step. You can record your whole screen, a single window or a region you
   draw.

   On a Mac you can also **narrate**: turn on the toolbar's microphone and
   say what you're doing and why. Your speech is transcribed on the Mac and
   split across the steps by when you said it (speech just before a click
   goes with that click), so each step's description starts as your own
   words. With AI on, the narration is extra context for the step and guide
   text. Settings → Recording can also keep the audio in the guide's folder.
2. **Edit.** When you stop, the guide opens in the editor. Each step has a
   title and description (written for you from what you clicked, e.g. "Click
   **Save**"), and you can:
   - reword, reorder, merge, split or delete steps, and add sections and notes;
   - **Re-capture** a step when the app has changed (in the step's ⋯ menu or
     its right-click menu): a short recording starts on the same screen,
     window or region the guide was recorded on, and your next click,
     shortcut or manual step replaces the step's screenshots, click point,
     element and keystrokes. The step's text and your edits stay; if it has
     annotations, you choose whether to move them by how far the click
     moved, clear them or keep them as they are;
   - **Insert recorded steps here** (same menus, or Insert → Recorded
     steps…) to record new steps into the middle of a guide;
   - annotate screenshots with arrows, boxes, highlights, callouts and blurs,
     and switch between a close-up crop and the full screenshot;
   - review blurs suggested for sensitive text found in the screenshots
     (see Privacy below);
   - undo and redo everything; changes save automatically;
   - go back to an earlier version in **Version history**: a version is
     saved when you close the guide, before each **Write with AI** run, before
     an MCP edit and whenever you click **Save version** (with a name if you
     like). See what changed as a text diff, preview it with its screenshots,
     and restore it (the guide as it is now is kept as a version first).
     Settings → General sets how many versions to keep (50 and 90 days by
     default; named versions are always kept).
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

   For a chat, a ticket, a README or a training session, export a **video
   (MP4)** or an **animated GIF** instead: each step is shown for a few
   seconds, zooming smoothly into where you clicked, with a cursor, a click
   marker and the step's title as a caption. Choose the seconds per step,
   the frame (wide 16:9, square or vertical), and whether to zoom and show
   captions. Video is encoded on your Mac with AVFoundation. **Slides
   (PPTX)** give one slide per step with its image, title and description,
   and the description in the speaker notes.

   To hand the task to an AI agent, export an **agent skill**: a folder with
   `SKILL.md` (the steps as instructions, with the purpose, prerequisites and
   expected result, in the [Agent Skills](https://docs.anthropic.com/en/docs/agents-and-tools/agent-skills/overview)
   format), `steps.json` (each step's element locator: app, window, role and
   label, plus the click point and the keys) and the step images, blurs
   burned in. Optionally it includes `replay.js`, a macOS script that repeats
   the steps by finding each element through the accessibility API:
   `osascript -l JavaScript replay.js --dry-run` reports what it would do and
   whether each element is there, without touching anything. The script stops
   at any step it can't do safely (an element it can't find or tell apart, a
   password, a double-click, a drag or a manual step); do that one by hand and
   continue with `--from`. Steps whose element wasn't identified clearly (no
   label, a generic role such as a group, no app or window) are flagged in the
   export dialog, in the step inspector, where you can name the element, and in
   `steps.json` and `SKILL.md`.

   For a learning management system (LMS), export a **SCORM course**: a
   SCORM 1.2 `.zip` with an `imsmanifest.xml` and the interactive
   walkthrough, which tells the LMS where the learner is (and resumes there)
   and marks the course completed once they reach the last step. Add a
   short quiz for the end if you like, one line per question followed by
   its choices (`- [x]` for the right one, `- [ ]` for the others); its score
   is reported to the LMS, and with the "Quiz passed" rule the course is
   passed or failed against the mastery score instead. Choose the course
   title and identifier; the identifier stays the same for every export of a
   guide by default, so the LMS sees a new version of the course. The quiz is
   kept with the export options, not in the guide. The package also works
   opened on its own, without an LMS.

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

## Sync with the web

On Pro and Enterprise, guides sync with your workspace in the GuideRecorder
web app, where your team can read, edit, review and share them. Click
**Sign in to sync** at the bottom of the library's sidebar: your browser opens,
you sign in there and approve the code the app shows, and the app is signed in
(the session token is kept in the Keychain). Then pick the workspace and
either sync the whole library or turn on single guides (right-click a guide →
**Sync to Web**). The sidebar shows when it last synced; Settings › Sync has
the rest, including signing out.

Syncing runs every 30 seconds, when the window comes to the front and soon
after you edit a guide. Blurred areas are burned into the screenshots that are
uploaded. If a guide changed both here and on the web, the changes are merged;
where they clash, you keep yours, the web's, or both.

## Privacy

GuideRecorder is built to be safe to leave running:

- **Local-first.** Guides are ordinary folders on your disk (one
  `.guiderecorder` folder per guide, holding a `guide.json`, its images and
  a `.history/` folder of saved versions),
  so they work with any backup or sync tool. Nothing is uploaded unless you
  sign in to sync (below), and then only the guides you choose.
- **Password fields are never recorded.** Keystrokes in secure fields are
  masked before they are stored.
- **Excluded apps.** While an app on your exclusion list is in front, nothing
  from it is captured: no screenshots, keys or clicks. Its windows are also
  kept out of every screenshot, even behind other apps. By default this covers
  1Password, Bitwarden, Keychain Access, Passwords and Messages, and you can
  edit the list in Settings.
- **Sensitive text is found on your Mac.** When a recording stops, the text
  in its screenshots is read on the device (no key, no network) and checked
  for email addresses, phone numbers, card numbers, IBANs, IP addresses, API
  keys and tokens, long numbers, and your own words and patterns. Each
  finding becomes a suggested blur, or a blur straight away; Settings ›
  Privacy chooses, or turns it off. The editor's Privacy tab lists every
  finding to blur or keep, and which steps were scanned. Export asks you to
  review suggestions left; the MCP server and the command line, which ask no
  one, blur them.
- **Narration is heard and transcribed on your Mac.** The microphone is off unless you turn it
  on in the toolbar, and only heard while a recording runs (not while
  paused). Speech is recognised on the device (Apple's Speech framework,
  on-device only): if your language can't be recognised offline, narration
  says so rather than use Apple's servers. The audio is kept only if you ask
  (`narration.m4a` in the guide's folder), and sync never uploads it. The
  transcript is kept with each step, like its text (so it syncs with the
  guide), and with AI on it is sent with that step.
- **AI only when you say so.** Nothing is sent anywhere unless AI is turned
  on and you have added your own key, which is kept in the macOS Keychain.
  Screenshots are sent with their blurs, and the sensitive text found on your
  Mac, already blurred. Blur suggestions are only suggestions until you
  accept them.

## Requirements

- macOS 12.3 or later.
- Permissions, granted on first launch: **Screen Recording** (screenshots),
  **Accessibility** (which button you clicked) and **Input Monitoring**
  (clicks and keys).
- Optional, to narrate: **Microphone** and **Speech Recognition**, with your
  language's dictation available on the device.
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
cargo run -p guiderecorder-cli -- --library ~/Documents/GuideRecorder list --overdue
cargo run -p guiderecorder-cli -- --library ~/Documents/GuideRecorder search "api key"
cargo run -p guiderecorder-cli -- --library ~/Documents/GuideRecorder validate "<guide>.guiderecorder"
cargo run -p guiderecorder-cli -- --library ~/Documents/GuideRecorder --index <path> reindex
cargo run -p guiderecorder-cli -- --library ~/Documents/GuideRecorder export "<guide>.guiderecorder" --format pdf --out guide.pdf --paper letter
```

`export` takes `--format md|html|walkthrough|docx|pdf|pptx|mp4|gif|skill|scorm`
(`walkthrough` is an interactive click-through `guide.html`; `mp4` needs
macOS; `skill` is an agent skill folder; `scorm` is a SCORM 1.2 `.zip`),
`--out`, and optionally `--paper a4|letter` (DOCX and PDF; default A4),
`--seconds-per-step`, `--size wide|square|vertical`, `--no-zoom` and
`--no-captions` (MP4 and GIF), `--replay` (a skill's `replay.js`),
`--course-title`, `--course-id`, `--completion last-step|quiz-passed`,
`--mastery-score` (default 80) and `--quiz <file>` (SCORM), `--logo`, `--accent`,
`--review-details` (version, owner and last review day under the summary),
`--overwrite` and `--strict`. See `guiderecorder export --help`.
`list --overdue` shows only guides whose review is past due.

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

### The web app

The web app is in `web/` (see [web/README.md](web/README.md)). In development
it runs at `https://app.guiderecorder.dev:3000`, and development builds of the
desktop app sign in to it. Set that up once:

```sh
scripts/dev-domain.sh setup    # trusts a local CA (mkcert), makes the certificate,
                               # adds the hosts entry; asks for your password
```

Then start it with the "Web" process in [Solo](https://soloterm.com)
(`solo.yml`), or by hand as in web/README.md. The address the app signs in to
comes from `app/.env` (release builds: `https://gr.dufflebear.dev`) and
`app/.env.development`; override it in `app/.env.local`.

### The mobile app

The iPhone and iPad app is in `mobile/` (see [mobile/README.md](mobile/README.md)):
it reads a workspace's synced guides and adds steps to them with photos from
the front or back camera. `cd mobile && pnpm start`, then open it in the iOS
simulator or Expo Go.

### Project layout

```
crates/
  guiderecorder-core/      guide format, library, search index, step detection, export, AI
  guiderecorder-capture/   screen and input capture (macOS backend; a stub elsewhere)
  guiderecorder-mcp/       read-only MCP server (binary: guiderecorder-mcp)
  guiderecorder-cli/       command line tool (binary: guiderecorder)
app/                       desktop app: Tauri 2, React, TypeScript, Tailwind
  src-tauri/               the app's Rust side (guiderecorder-app)
web/                       web app: TanStack Start, Better Auth, Drizzle, Postgres, S3
mobile/                    iPhone and iPad app: Expo, React Native (see mobile/README.md)
packages/                  shared by app, web and mobile: guide schema, sync merge, UI
fixtures/guides/           sample guides used by the tests
docs/                      signing, releasing, capture smoke test, dependency licences
```

### Tests

```sh
cargo test --workspace                 # Rust, from the repo root
cd app && pnpm test && pnpm e2e        # frontend unit and end-to-end tests
pnpm --filter @guiderecorder/web test  # web app
```

CI runs these on every pull request, the Rust tests on both macOS and
Windows. To publish a release, see [docs/releasing.md](docs/releasing.md).

## Licence

GuideRecorder is released under the [MIT License](LICENSE). Third-party
licences are listed in [docs/licences.md](docs/licences.md).
