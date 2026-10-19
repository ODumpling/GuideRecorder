//! HTML export (#363, design #443).
//!
//! Goldens are under `tests/golden/html/`, one per fixture guide, with the
//! base64 image data replaced by `…` (PNG bytes may differ by platform);
//! regenerate them with
//! `UPDATE_GOLDEN=1 cargo test -p guiderecorder-core --test export_html` and
//! review the diff.

mod support;

use std::fs;

use guiderecorder_core::export::html::{self, Branding, HtmlOptions, CSP};
use guiderecorder_core::export::md::{self, BrandingError, ExportError};
use guiderecorder_core::export::{ExportReport, RenderGuide, RenderWarning};
use guiderecorder_core::library::{GuideHandle, Library, PixelBuffer, StepImageVariant};
use guiderecorder_core::model::*;
use guiderecorder_core::Ulid;
use support::{assert_golden_text, copy_dir, fixture_guides_dir};

// ---------------------------------------------------------------------------
// Helpers

struct Fixtures {
    _dir: tempfile::TempDir,
    guides: Vec<(GuideHandle, Guide)>,
}

/// A temp copy of the fixture guides, opened read-only.
fn fixtures() -> Fixtures {
    let dir = tempfile::tempdir().unwrap();
    for entry in fs::read_dir(fixture_guides_dir()).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "guiderecorder") {
            copy_dir(&path, &dir.path().join(path.file_name().unwrap()));
        }
    }
    let library = Library::open(dir.path()).unwrap();
    let mut guides: Vec<_> = library
        .list()
        .unwrap()
        .into_iter()
        .map(|summary| {
            let loaded = library.read(&summary.path).unwrap();
            (loaded.handle, loaded.guide)
        })
        .collect();
    guides.sort_by(|a, b| a.1.title.cmp(&b.1.title));
    assert!(!guides.is_empty());
    Fixtures { _dir: dir, guides }
}

fn render(guide: &Guide, handle: &GuideHandle, opts: &HtmlOptions) -> (String, ExportReport) {
    html::render_string(&RenderGuide::new(guide, handle), handle, opts).unwrap()
}

/// Replaces the payload of every base64 data URI with `…`.
fn strip_data(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(i) = rest.find(";base64,") {
        let (head, tail) = rest.split_at(i + ";base64,".len());
        out.push_str(head);
        out.push('…');
        let end = tail
            .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=')))
            .unwrap_or(tail.len());
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

#[derive(Debug)]
struct TagInfo {
    name: String,
    attrs: Vec<(String, String)>,
}

/// Every tag in `html`. Panics on a `<` that does not start a well-formed
/// tag: escaped text never contains one.
fn tags(html: &str) -> Vec<TagInfo> {
    let mut out = Vec::new();
    let mut rest = html;
    while let Some(i) = rest.find('<') {
        rest = &rest[i + 1..];
        let end = rest.find('>').expect("unclosed tag");
        let inner = &rest[..end];
        rest = &rest[end + 1..];
        let inner = inner.strip_prefix('/').unwrap_or(inner);
        let name_end = inner
            .find(|c: char| c.is_whitespace())
            .unwrap_or(inner.len());
        let name = inner[..name_end].to_ascii_lowercase();
        assert!(
            !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '!'),
            "bad tag {inner:?}"
        );
        let mut attrs = Vec::new();
        let mut a = inner[name_end..].trim_start();
        while !a.is_empty() {
            let key_end = a.find(['=', ' ']).unwrap_or(a.len());
            let key = a[..key_end].to_ascii_lowercase();
            a = &a[key_end..];
            let mut value = String::new();
            if let Some(v) = a.strip_prefix("=\"") {
                let close = v.find('"').expect("unterminated attribute");
                value = v[..close].to_owned();
                a = &v[close + 1..];
            } else {
                assert!(!a.starts_with('='), "unquoted attribute in {inner:?}");
            }
            attrs.push((key, value));
            a = a.trim_start();
        }
        out.push(TagInfo { name, attrs });
    }
    out
}

const ALLOWED_TAGS: &[&str] = &[
    "!doctype", "html", "head", "meta", "title", "style", "body", "main", "header", "section",
    "aside", "article", "figure", "div", "span", "img", "h1", "h2", "h3", "p", "ul", "ol", "li",
    "strong", "em", "code", "a", "br",
];
const ALLOWED_ATTRS: &[&str] = &[
    "html",
    "lang",
    "charset",
    "http-equiv",
    "content",
    "name",
    "class",
    "id",
    "role",
    "aria-label",
    "aria-describedby",
    "aria-hidden",
    "tabindex",
    "style",
    "src",
    "width",
    "height",
    "alt",
    "href",
    "title",
    "rel",
    "start",
];

/// Checks the structural safety of `html`: only allowed tags and
/// attributes, images only from data URIs, the CSP present.
fn assert_safe(html: &str) {
    let tags = tags(html);
    for tag in &tags {
        assert!(ALLOWED_TAGS.contains(&tag.name.as_str()), "tag {tag:?}");
        for (key, value) in &tag.attrs {
            assert!(
                ALLOWED_ATTRS.contains(&key.as_str()),
                "attribute {key} in {tag:?}"
            );
            if key == "src" {
                assert!(value.starts_with("data:image/"), "src {value:?}");
            }
            if key == "style" {
                assert_safe_style(value);
            }
            if key == "href" {
                let lower = value.to_ascii_lowercase();
                assert!(
                    lower.starts_with("https://")
                        || lower.starts_with("http://")
                        || lower.starts_with("mailto:")
                        || !lower.contains(':'),
                    "href {value:?}"
                );
            }
        }
    }
    let csp = tags
        .iter()
        .find(|t| {
            t.attrs
                .iter()
                .any(|(k, v)| k == "http-equiv" && v == "Content-Security-Policy")
        })
        .expect("CSP meta tag");
    let content = &csp.attrs.iter().find(|(k, _)| k == "content").unwrap().1;
    assert_eq!(content.replace("&#39;", "'").replace("&#x27;", "'"), CSP);
    assert!(CSP.contains("default-src 'none'") && !CSP.contains("script-src"));

    // Resources: the tag allowlist has no <link>, <script>, <iframe> or
    // <object>, every src is a data URI (checked above), and the CSS
    // imports nothing. URLs elsewhere are link hrefs or escaped text.
    let lower = html.to_ascii_lowercase();
    assert!(!lower.contains("url("), "css url()");
    assert!(!lower.contains("@import"));
    assert!(!lower.contains("<script"));
}

/// Inline styles hold only hotspot positions (`left:12.5%`) and a colour
/// (`--hs:#16a34a`, `--hs:red`).
fn assert_safe_style(style: &str) {
    for declaration in style.split(';') {
        let (property, value) = declaration.split_once(':').expect(style);
        let ok = match property {
            "left" | "top" | "width" | "height" => value
                .strip_suffix('%')
                .is_some_and(|n| n.parse::<f64>().is_ok_and(|n| (0.0..=100.0).contains(&n))),
            "--hs" => value
                .strip_prefix('#')
                .map_or(value.bytes().all(|b| b.is_ascii_alphabetic()), |hex| {
                    hex.bytes().all(|b| b.is_ascii_hexdigit())
                }),
            _ => false,
        };
        assert!(ok, "style {style:?}");
    }
}

// ---------------------------------------------------------------------------
// Fixtures

#[test]
fn fixture_goldens() {
    let fixtures = fixtures();
    for (handle, guide) in &fixtures.guides {
        let (html, report) = render(guide, handle, &HtmlOptions::default());
        assert_safe(&html);
        let mut golden = strip_data(&html);
        golden.push_str("<!-- warnings\n");
        for (id, warning) in &report.warnings {
            golden.push_str(&format!("{id} {warning}\n"));
        }
        golden.push_str("-->\n");
        assert_golden_text(&format!("html/{}.golden", md::slug(&guide.title)), &golden);
    }
}

#[test]
fn export_writes_one_self_contained_file() {
    let fixtures = fixtures();
    let (handle, guide) = &fixtures.guides[0];
    let out = tempfile::tempdir().unwrap();
    let render_guide = RenderGuide::new(guide, handle);
    let report = html::export(&render_guide, handle, out.path(), &HtmlOptions::default()).unwrap();
    assert_eq!(report.files, [out.path().join("guide.html")]);
    let entries: Vec<_> = fs::read_dir(out.path()).unwrap().collect();
    assert_eq!(entries.len(), 1);

    let written = fs::read_to_string(&report.files[0]).unwrap();
    let (string, _) = render(guide, handle, &HtmlOptions::default());
    assert_eq!(written, string);
    // One embedded image per step.
    assert_eq!(
        written.matches("src=\"data:image/png;base64,").count(),
        guide.steps().count()
    );

    let err = html::export(&render_guide, handle, out.path(), &HtmlOptions::default()).unwrap_err();
    assert!(matches!(err, ExportError::NotEmpty(_)), "{err}");
    let opts = HtmlOptions {
        overwrite: true,
        ..HtmlOptions::default()
    };
    html::export(&render_guide, handle, out.path(), &opts).unwrap();
}

#[test]
fn hotspots_overlay_an_unmarked_image() {
    let fixtures = fixtures();
    let (handle, guide) = fixtures
        .guides
        .iter()
        .find(|(_, g)| g.kind == GuideKind::Annotated)
        .expect("an annotation guide fixture");
    let (html, report) = render(guide, handle, &HtmlOptions::default());
    assert_safe(&html);
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert!(!html.to_ascii_lowercase().contains("<script"));

    // One focusable overlay per hotspot, with its popover side.
    let hotspots: Vec<TagInfo> = tags(&html)
        .into_iter()
        .filter(|t| {
            t.attrs
                .iter()
                .any(|(k, v)| k == "class" && v.starts_with("hotspot hotspot-"))
        })
        .collect();
    let attr = |tag: &TagInfo, key: &str| {
        tag.attrs
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    };
    let summary: Vec<(String, String)> = hotspots
        .iter()
        .map(|t| (attr(t, "aria-label"), attr(t, "class")))
        .collect();
    let expected = [
        ("1. Reception", "hotspot hotspot-below hotspot-start"),
        (
            "2. Kitchen &amp; café",
            "hotspot hotspot-below hotspot-center",
        ),
        (
            "3. Meeting room &quot;Fjord&quot;",
            "hotspot hotspot-below hotspot-end",
        ),
        ("4. Quiet zone", "hotspot hotspot-above hotspot-start"),
        ("1. Stairwell B", "hotspot hotspot-above hotspot-end"),
        ("2. Stairwell A", "hotspot hotspot-above hotspot-start"),
    ]
    .map(|(a, b)| (a.to_owned(), b.to_owned()));
    assert_eq!(summary, expected);
    for tag in &hotspots {
        assert_eq!(attr(tag, "tabindex"), "0");
        assert_eq!(attr(tag, "role"), "button");
        let described = attr(tag, "aria-describedby");
        assert!(html.contains(&format!("id=\"{described}\"")), "{described}");
    }
    // Reception sits at (24, 24) 140×92 on a 480×300 page.
    assert_eq!(
        attr(&hotspots[0], "style"),
        "left:5%;top:8%;width:29.167%;height:30.667%"
    );
    assert!(attr(&hotspots[4], "style").ends_with(";--hs:#16a34a"));

    // Text is escaped, line breaks become <br>, and the legend repeats it.
    assert!(html.contains(
        "<span class=\"hotspot-text\" id=\"hotspot-1-1-text\">Sign in here when you arrive.<br>\
         Visitor badges are in the top drawer.</span>"
    ));
    assert!(html.contains("press &lt;Input&gt; on the remote"));
    assert_eq!(html.matches("<ol class=\"hotspot-legend\">").count(), 2);
    assert!(html.contains("<li id=\"hotspot-1-3\"><span class=\"hotspot-number\">3</span>"));
    // Pages, not steps.
    assert!(html.contains("aria-label=\"Page 1\""));

    // The embedded image has no markers baked in.
    let render_guide = RenderGuide::new(guide, handle);
    let page = render_guide.steps().next().unwrap();
    let unmarked = render_guide.step_image(
        page,
        &guiderecorder_core::export::ImageOptions {
            hotspot_markers: false,
            ..HtmlOptions::default().image
        },
    );
    let marked = render_guide.step_image(page, &HtmlOptions::default().image);
    assert_ne!(unmarked.png, marked.png);
    let first_src = html
        .split("src=\"data:image/png;base64,")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .unwrap();
    assert_eq!(first_src, base64(&unmarked.png));
}

/// Standard base64 with padding, to compare embedded images.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Synthetic guides

const HOSTILE: &[&str] = &[
    "<script>alert(1)</script>",
    "\"><img src=x onerror=alert(1)>",
    "'><svg onload=alert(1)>",
    "</title></style><script>alert(1)</script>",
    "[click](javascript:alert(1)) [x](JaVaScRiPt:alert(1)) [y](data:text/html,<b>)",
    "<a href=\"https://evil.example\">evil</a>",
    "<img src=\"https://evil.example/pixel.png\">",
    "{{ 7*7 }} {% if true %}x{% endif %}",
    "&lt;b&gt; &amp;amp; &#60;script&#62;",
    "`<code>` **<b>bold</b>** *<i>em</i>*",
    "[ok](https://example.com/?a=1&b=\"2\"><script> \"t\\\"><b>\")",
];

fn hostile_guide(library: &Library) -> (GuideHandle, Guide) {
    let mut guide = Guide::new(HOSTILE[3], "en\"><script>");
    guide.summary = HOSTILE[1].into();
    guide.intro = HOSTILE.join("\n\n");
    guide.prerequisites = HOSTILE
        .iter()
        .map(|t| format!("- {t}"))
        .collect::<Vec<_>>()
        .join("\n");
    let handle = library.create(&guide).unwrap();

    let real = Ulid::new();
    let pixels = vec![200u8; 30 * 10 * 4];
    let full = handle
        .write_step_image(
            real,
            StepImageVariant::Full,
            &PixelBuffer::rgba(30, 10, &pixels),
        )
        .unwrap();

    let hotspots: Vec<Annotation> = HOSTILE
        .iter()
        .enumerate()
        .map(|(i, text)| Annotation {
            id: Ulid::new(),
            shape: Shape::Hotspot(HotspotGeometry {
                rect: Rect {
                    x: i as f64 * 2.0,
                    y: 1.0,
                    width: 3.0,
                    height: 5.0,
                },
                title: (*text).into(),
                description: format!("{text}\n{text}"),
            }),
            style: AnnotationStyle::default(),
        })
        .collect();
    let mut items = Vec::new();
    for (i, text) in HOSTILE.iter().enumerate() {
        items.push(Item::Section(Section {
            id: Ulid::new(),
            title: (*text).into(),
        }));
        let id = if i == 0 { real } else { Ulid::new() };
        items.push(Item::Step(Step {
            id,
            title: (*text).into(),
            description: (*text).into(),
            kind: StepKind::Click,
            point: None,
            end_point: None,
            element: None,
            keystrokes: None,
            image: StepImage {
                full: if i == 0 {
                    full.clone()
                } else {
                    format!("steps/{id}.webp")
                },
                crop: None,
                crop_rect: None,
                after: None,
                show: ImageShow::Full,
            },
            annotations: if i == 0 { hotspots.clone() } else { Vec::new() },
            ai_state: AiState::default(),
        }));
        items.push(Item::Note(Note {
            id: Ulid::new(),
            kind: [NoteKind::Tip, NoteKind::Warning, NoteKind::Note][i % 3],
            body: (*text).into(),
        }));
    }
    guide.items = items;
    library.save_to(&handle, &guide).unwrap();
    (handle, guide)
}

#[test]
fn hostile_text_is_escaped() {
    let dir = tempfile::tempdir().unwrap();
    let library = Library::open(dir.path()).unwrap();
    let (handle, guide) = hostile_guide(&library);
    let (html, _) = render(&guide, &handle, &HtmlOptions::default());
    assert_safe(&html);
    // Unsafe links survive only as escaped text, never as an href or src.
    assert!(html.contains("[click](javascript:alert(1))"));
    // Guide text is data, never template source.
    assert!(!html.contains("49 x"), "template expression evaluated");
    assert!(html.contains("{{ 7*7 }} {% if true %}x{% endif %}"));
    // The only link is the safe one, with its hostile query escaped.
    let hrefs: Vec<String> = tags(&html)
        .into_iter()
        .filter(|t| t.name == "a")
        .flat_map(|t| t.attrs.into_iter().filter(|(k, _)| k == "href"))
        .map(|(_, v)| v)
        .collect();
    assert!(!hrefs.is_empty());
    for href in &hrefs {
        assert!(href.starts_with("https://example.com/"), "{href}");
    }
    assert!(html.starts_with("<!doctype html>\n<html lang=\"\">"));
    // Hostile hotspot text is escaped. (Hostile colours never get past the
    // renderer's check; see `PlacedHotspot::color`.)
    assert_eq!(
        html.matches("class=\"hotspot hotspot-").count(),
        HOSTILE.len()
    );
    assert!(!html.contains(";--hs:"));
    assert!(html.contains("aria-label=\"1. &lt;script&gt;alert(1)&lt;"));
    assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;<br>&lt;script&gt;"));
}

#[test]
fn placeholders_and_warnings_land_in_report() {
    let dir = tempfile::tempdir().unwrap();
    let library = Library::open(dir.path()).unwrap();
    let (handle, guide) = hostile_guide(&library);
    let (html, report) = render(&guide, &handle, &HtmlOptions::default());
    let steps: Vec<&Step> = guide.steps().collect();
    assert_eq!(report.warnings.len(), steps.len() - 1);
    assert!(report.files.is_empty());
    for ((id, warning), step) in report.warnings.iter().zip(&steps[1..]) {
        assert_eq!(*id, step.id);
        assert!(
            matches!(warning, RenderWarning::ImageUnavailable { .. }),
            "{warning:?}"
        );
    }
    assert!(html.contains("width=\"30\" height=\"10\""));
    assert_eq!(
        html.matches("width=\"800\" height=\"450\"").count(),
        steps.len() - 1
    );
}

#[test]
fn branding_is_off_by_default_and_validated() {
    let fixtures = fixtures();
    let (handle, guide) = &fixtures.guides[0];
    let (plain, _) = render(guide, handle, &HtmlOptions::default());
    assert!(!plain.contains("class=\"logo\""));
    assert_eq!(plain.matches("--accent:").count(), 1);

    let opts = HtmlOptions {
        branding: Branding {
            logo: Some(PNG_1X1.to_vec()),
            accent: Some("#C0FFEE".into()),
        }
        .check()
        .unwrap(),
        ..HtmlOptions::default()
    };
    let (branded, _) = render(guide, handle, &opts);
    assert_safe(&branded);
    // The re-encoded PNG, not the bytes as given.
    assert!(branded.contains("<img class=\"logo\" src=\"data:image/png;base64,iVBORw0KGgo"));
    assert!(branded.contains(":root { --accent: #c0ffee; }"));

    // Hostile accents and logos never reach the writer: only `check` makes
    // the branding the options take.
    let hostile = Branding {
        accent: Some("red;}</style><script>".into()),
        ..Branding::default()
    };
    assert!(matches!(hostile.check(), Err(BrandingError::Accent(_))));
    for logo in [&b"<svg/>"[..], b"\x89PNG fake", b"GIF89a"] {
        let bad = Branding {
            logo: Some(logo.to_vec()),
            ..Branding::default()
        };
        assert!(bad.check().is_err());
    }
}

#[test]
fn review_details_show_only_when_asked() {
    let fixtures = fixtures();
    let (handle, guide) = &fixtures.guides[0];
    let mut guide = guide.clone();
    guide.language = "en".into();
    guide.review = ReviewInfo {
        owner: Some("<b>Ops</b>".into()),
        version: Some("2.0".into()),
        last_reviewed: chrono::NaiveDate::from_ymd_opt(2026, 10, 1),
        ..ReviewInfo::default()
    };
    let (plain, _) = render(&guide, handle, &HtmlOptions::default());
    assert!(!plain.contains("class=\"details\""));
    let opts = HtmlOptions {
        review_details: true,
        ..HtmlOptions::default()
    };
    let (html, _) = render(&guide, handle, &opts);
    assert_safe(&html);
    let line = html
        .lines()
        .find(|l| l.contains("class=\"details\""))
        .unwrap();
    assert_eq!(
        line,
        "<p class=\"details\">Version: 2.0 · Owner: &lt;b&gt;Ops&lt;&#x2f;b&gt; · Last reviewed: \
         2026-10-01</p>"
    );
    // Nothing set: no line.
    guide.review = ReviewInfo::default();
    let (html, _) = render(&guide, handle, &opts);
    assert!(!html.contains("class=\"details\""));
}

#[test]
fn branded_fixture_goldens() {
    let fixtures = fixtures();
    let opts = HtmlOptions {
        branding: Branding {
            logo: Some(PNG_1X1.to_vec()),
            accent: Some("#aa1122".into()),
        }
        .check()
        .unwrap(),
        ..HtmlOptions::default()
    };
    for (handle, guide) in &fixtures.guides {
        let (html, _) = render(guide, handle, &opts);
        assert_safe(&html);
        assert_golden_text(
            &format!("html/branded/{}.golden", md::slug(&guide.title)),
            &strip_data(&html),
        );
    }
}

/// A valid 1×1 RGBA PNG.
const PNG_1X1: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0x63, 0x64, 0x60, 0xf8, 0x5f,
    0x0f, 0x00, 0x02, 0x87, 0x01, 0x80, 0xeb, 0x47, 0xba, 0x92, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45,
    0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];
