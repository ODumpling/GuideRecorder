# guiderecorder-mcp

An [MCP](https://modelcontextprotocol.io) server over a GuideRecorder
library, over stdio. It lets an agent such as Claude Code find and read your
recorded guides, including their screenshots, and, if you allow it with
`--writable`, edit them and create annotation guides. It works whether or not
the GuideRecorder app is running.

## Setup

Build it:

```sh
cargo build --release -p guiderecorder-mcp
```

Add it to Claude Code:

```sh
claude mcp add guiderecorder -- /path/to/target/release/guiderecorder-mcp --library ~/Documents/GuideRecorder
```

Pass `--library` the folder that holds your `.guiderecorder` bundles. You can set
`GUIDERECORDER_LIBRARY` instead of passing `--library`.

Options:

| Option | Default | |
| --- | --- | --- |
| `--library <dir>` | `$GUIDERECORDER_LIBRARY` | Library folder. It must already exist. |
| `--index <path>` | `~/Library/Application Support/dev.dufflebear.guiderecorder/index.sqlite` | The app's search index. |
| `--writable` | off | Also offer the [write tools](#writing---writable). |

In the app, Settings → MCP shows the command; "Allow MCP to edit guides"
adds `--writable` to it.

The server opens the index read-only. If the index is missing, stale or from
another version, the server scans the library folders instead, which gives
the same results more slowly.

## Read-only

Without `--writable` the server never writes to the library or the index:

- Guides are read with `Library::read`, which migrates older guides in memory
  only, and never with `load`.
- Guides saved by a newer GuideRecorder are shown read-only. If this version cannot
  decode one, you get an error asking you to update.
- Duplicate copies of a bundle (for example, a Finder copy) are hidden.
- The index is opened read-only and its database file is never modified.
  However, SQLite in WAL mode may create the `index.sqlite-shm` sidecar and an
  empty `index.sqlite-wal` next to it. This is normal SQLite behaviour; no
  data is written to either.

## Tools

| Tool | Arguments | Returns |
| --- | --- | --- |
| `list_guides` | `query?`, `tag?`, `limit?` (50, max 500) | `id`, `title`, `summary`, `tags`, `step_count`, `updated_at`, `read_only` and `folder` (the library subfolder, `""` at the top level) for each guide, plus `total`. Also `owner`, `status`, `version`, `last_reviewed`, `review_interval_months`, `review_due` (null when unset) and `review_overdue`. |
| `get_guide` | `id`, `include_images?` (false), `image_steps?`, `max_images?` (10, max 25) | The guide as Markdown (with its owner, status, version and review dates when set), and optionally step screenshots (at most about 5 MB of images). |
| `get_step` | `guide_id`, `step_id`, `image?` (`"crop"`, `"full"` or `"none"`; default `"crop"`) | The step's text, element, keys and screenshot. |
| `search_steps` | `query`, `limit?` (20, max 100) | Hits with `guide_id`, `guide_title`, `kind`, `item_id`, `step_number`, `title` and `snippet` (matches in `**bold**`). |

`read_only` is true when the GuideRecorder app won't edit the guide as it is. That
happens when it was saved by a newer GuideRecorder, or when it uses an older format
that the app upgrades the next time it opens the guide. Read-only guides can
still be read with `get_guide` and `get_step`.

JSON keys in tool results are snake_case on purpose, following MCP tool
conventions. The CLI's `--json` output uses camelCase to match `guide.json`.
Don't "fix" either one to match the other.

Guide Markdown gives every item an anchor that you can pass back as an id.
`step_id` accepts the id with or without the `step-` prefix:

```markdown
# Rotate an API key {#guide-01M4WZCE2000000000000002R0}

## Setup {#section-…}

### 3. Click **Create key** and copy the new key {#step-01M4WZCE2000000000000002R8}

> **Warning** {#note-…}
```

Text from a guide is escaped, so it can't add headings, anchors or links of
its own. Typed keystrokes appear exactly as recorded, so password fields stay
masked.

### Images

Screenshots are rendered with `guiderecorder_core::export::render_step_image`.
Annotations are drawn, and blur and pixelate regions are burned in before the
image is scaled down. The images are PNG. The raw WebP screenshots never leave
the server.

- `get_step` scales its single image to at most 1280 px wide.
- `get_guide` scales its images to at most 1024 px wide. It stops adding
  images once their PNG bytes reach about 5 MB (before base64) or
  `max_images`, whichever comes first. A closing note says
  "Showing N of M step images" and how to get the rest.

If a render had problems, the image's text block says so. For example, a
missing file becomes a placeholder, and a crop whose position is unknown
falls back to the full screenshot so that redactions stay in place.

## Writing (`--writable`)

With `--writable` these tools are listed too. Without it they are neither
listed nor routed.

| Tool | Arguments | Returns |
| --- | --- | --- |
| `update_guide_fields` | `guide_id`, `title?`, `intro?`, `prerequisites?`, `summary?`, `tags?` (replaces the list) | `changed_fields`. |
| `update_step` | `guide_id`, `step_id`, `title?`, `description?` | `step_id`. |
| `add_note` | `guide_id`, `body`, `kind?` (`"tip"`, `"warning"` or `"note"`; default `"note"`), `after_id?` or `before_id?` (default: at the end) | `note_id`. |
| `reorder_items` | `guide_id`, `item_ids` (every item, once, in the new order) | |
| `add_tags` / `remove_tags` | `guide_id`, `tags` (case-insensitive) | `added` / `removed`, and `tags`. |
| `create_guide_from_images` | `paths` (absolute; PNG, JPEG or WebP; at most 100), `title?` (default: the first file's name) | `guide_id`, `title`, `folder`, `step_count`, `step_ids`. |

Every edit also returns `guide_id`, `changed` (false when nothing was
different; then nothing is written) and `updated_at`. Item ids may keep
their `step-`, `section-` or `note-` anchor prefix.

How edits are made (see `guiderecorder_core::edit`):

- **Atomically.** Edits go through `Library::edit`, which reads the guide,
  changes a copy and writes `guide.json` with `Library::save_to` (temp file,
  fsync, rename). If `guide.json` changed on disk meanwhile, the edit is
  refused. Edits run one at a time.
- **Marked as the user's.** An edit through MCP is an explicit edit by the
  user, so it marks text as the editor does: a changed guide-level field joins
  `editedFields` and the guide's `aiState` becomes `edited`; a changed step
  title or description makes the step `edited`. GuideRecorder's AI passes then
  leave that text alone. Text already marked edited can still be changed.
  Adding notes and reordering mark nothing, as in the editor.
- **Not while open in the app.** The app's editor saves a guide whole, so it
  would overwrite an edit made while the guide is open. While a guide is open,
  the app keeps a `.editor.lock` file in its bundle holding the app's process
  id, and edits are refused while that process runs. A lock left behind by a
  crash names a process that has exited, and is ignored. The lock is checked
  before the read and again before the write; an app that opens the guide
  during the write itself could still load the old copy.
- **Not read-only guides.** Guides saved by a newer GuideRecorder are
  refused, and so are guides in an older format until the app has opened
  (and upgraded) them once.
- **The title keeps the folder.** As in the editor, a new title doesn't
  rename the bundle folder.
- **The index is not written.** The app indexes changes at its next sync.
  Until then, this server's reads see the index as stale and scan the
  library instead.
- `create_guide_from_images` makes the guide the app's "New annotation
  guide…" makes (`guiderecorder_core::pages`), in the library's top folder:
  one page per image, titled after its file, scaled down to at most 4000 px.
  A file that fails leaves no guide behind. PDFs are only supported in the
  app.

There is no undo yet; version history (#25) will add it.

## Resources

- `guiderecorder://guide/{id}`: a guide as Markdown, the same text as `get_guide`.
  `resources/list` lists every guide.

## Errors

Problems you can fix are returned as tool errors with an explanation: an
invalid or unknown id, a section or note id passed as a step, an empty
query, or a guide from a newer version.

For resources:

- An unknown URI, an invalid id or an id that isn't in the library returns
  `resource not found` (-32002).
- A guide that exists but can't be read (for example, it's from a newer
  version or its JSON is corrupt) returns an internal error (-32603) with the
  same explanation.

## Tests

```sh
cargo test -p guiderecorder-mcp
```

The tests run the server in-process. A JSON-RPC client talks to it over a
tokio duplex, using a copy of `fixtures/guides` and an index built from that
copy. They cover every tool and resource, the errors, image equality with
`render_step_image`, a burned-in blur, the image byte budget on large
screenshots, and that the library and index bytes are unchanged after every
read-only call. The SQLite `-shm`/`-wal` sidecars are the stated exception:
the `-wal` file must stay empty. The write tests check what is saved, the
edited markers, and that refused and no-op edits change no file.
