# guidesya-mcp

A read-only [MCP](https://modelcontextprotocol.io) server over a GuidesYa
library, over stdio. It lets an agent such as Claude Code find and read your
recorded guides, including their screenshots. It works whether or not the
GuidesYa app is running.

## Setup

Build it:

```sh
cargo build --release -p guidesya-mcp
```

Add it to Claude Code:

```sh
claude mcp add guidesya -- /path/to/target/release/guidesya-mcp --library ~/Documents/GuidesYa
```

Pass `--library` the folder that holds your `.guidesya` bundles. You can set
`GUIDESYA_LIBRARY` instead of passing `--library`.

Options:

| Option | Default | |
| --- | --- | --- |
| `--library <dir>` | `$GUIDESYA_LIBRARY` | Library folder. It must already exist. |
| `--index <path>` | `~/Library/Application Support/dev.guidesya.app/index.sqlite` | The app's search index. |

The server opens the index read-only. If the index is missing, stale or from
another version, the server scans the library folders instead, which gives
the same results more slowly.

## Read-only

The server never writes to the library or the index:

- Guides are read with `Library::read`, which migrates older guides in memory
  only, and never with `load`.
- Guides saved by a newer GuidesYa are shown read-only. If this version cannot
  decode one, you get an error asking you to update.
- Duplicate copies of a bundle (for example, a Finder copy) are hidden.
- The index is opened read-only and its database file is never modified.
  However, SQLite in WAL mode may create the `index.sqlite-shm` sidecar and an
  empty `index.sqlite-wal` next to it. This is normal SQLite behaviour; no
  data is written to either.

## Tools

| Tool | Arguments | Returns |
| --- | --- | --- |
| `list_guides` | `query?`, `tag?`, `limit?` (50, max 500) | `id`, `title`, `summary`, `tags`, `step_count`, `updated_at` and `read_only` for each guide, plus `total`. |
| `get_guide` | `id`, `include_images?` (false), `image_steps?`, `max_images?` (10, max 25) | The guide as Markdown, and optionally step screenshots (at most about 5 MB of images). |
| `get_step` | `guide_id`, `step_id`, `image?` (`"crop"`, `"full"` or `"none"`; default `"crop"`) | The step's text, element, keys and screenshot. |
| `search_steps` | `query`, `limit?` (20, max 100) | Hits with `guide_id`, `guide_title`, `kind`, `item_id`, `step_number`, `title` and `snippet` (matches in `**bold**`). |

`read_only` is true when the GuidesYa app won't edit the guide as it is. That
happens when it was saved by a newer GuidesYa, or when it uses an older format
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

Screenshots are rendered with `guidesya_core::export::render_step_image`.
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

## Resources

- `guidesya://guide/{id}`: a guide as Markdown, the same text as `get_guide`.
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
cargo test -p guidesya-mcp
```

The tests run the server in-process. A JSON-RPC client talks to it over a
tokio duplex, using a copy of `fixtures/guides` and an index built from that
copy. They cover every tool and resource, the errors, image equality with
`render_step_image`, a burned-in blur, the image byte budget on large
screenshots, and that the library and index bytes are unchanged after every
call. The SQLite `-shm`/`-wal` sidecars are the stated exception: the `-wal`
file must stay empty.
