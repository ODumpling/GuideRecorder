//! Arrow, box, highlight, callout and hotspot annotations as SVG, rendered
//! with usvg + resvg onto a tiny-skia pixmap.
//!
//! The SVG is generated here from typed geometry; every user string (callout
//! text, colours) is escaped, and colours are checked against a strict
//! syntax. External images are never resolved. Text uses only the bundled
//! Lato font (`assets/fonts`, SIL Open Font License 1.1), never system fonts,
//! so output is the same on every machine. Glyphs Lato lacks (e.g. CJK)
//! are not drawn.
//!
//! A hotspot is drawn as a marker like the editor draws it: its outline
//! (dashed while it has no title) over a faint tint, and a numbered badge
//! with a white ring inside its top-left corner (moved as needed to stay in
//! view). Its title and description are not drawn; writers list them in a
//! legend under the image. The badge is sized from the view's
//! width, so it keeps its share of the image when the image is scaled down
//! for export, but no larger than the hotspot (as in the editor). Markers
//! are a layer of their own ([`Layer::Hotspots`]), drawn over everything
//! else, blur included, as the editor shows them.

use std::fmt::Write as _;
use std::sync::{Arc, OnceLock};

use resvg::tiny_skia::{PixmapMut, Transform};
use resvg::usvg::{self, fontdb, ImageHrefResolver, Node};
use ulid::Ulid;

use super::raster::RenderWarning;
use crate::model::{
    Annotation, AnnotationStyle, ArrowGeometry, CalloutGeometry, HotspotGeometry, Point, Rect,
    Shape,
};

/// Lato Regular and Bold, SIL Open Font License 1.1 (see
/// `assets/fonts/Lato-OFL.txt`).
const LATO: &[u8] = include_bytes!("../../assets/fonts/Lato-Regular.ttf");
const LATO_BOLD: &[u8] = include_bytes!("../../assets/fonts/Lato-Bold.ttf");
pub(super) const FONT_FAMILY: &str = "Lato";

pub const DEFAULT_COLOR: &str = "#ff3b30";
pub const DEFAULT_HIGHLIGHT: &str = "#ffcc00";
const DEFAULT_HIGHLIGHT_OPACITY: f64 = 0.35;
const DEFAULT_STROKE: f64 = 4.0;
const DEFAULT_CALLOUT_STROKE: f64 = 2.0;
const DEFAULT_CALLOUT_FILL: &str = "#ffffff";
const CALLOUT_TEXT: &str = "#1d1d1f";
const DEFAULT_FONT_SIZE: f64 = 16.0;
const LINE_HEIGHT: f64 = 1.25;
/// Hotspot colour when the style has none: the editor's hotspot blue, kept
/// apart from the red of the other annotations.
pub const DEFAULT_HOTSPOT_COLOR: &str = "#007aff";
const HOTSPOT_TINT_OPACITY: f64 = 0.12;
const HOTSPOT_BADGE_TEXT: &str = "#ffffff";
/// Badge radius as a share of the view width, and its bounds in pixels.
const HOTSPOT_BADGE_RATIO: f64 = 1.0 / 64.0;
const HOTSPOT_BADGE_MIN: f64 = 10.0;
const HOTSPOT_BADGE_MAX: f64 = 48.0;

fn fontdb() -> Arc<fontdb::Database> {
    static DB: OnceLock<Arc<fontdb::Database>> = OnceLock::new();
    DB.get_or_init(|| {
        let mut db = fontdb::Database::new();
        db.load_font_data(LATO.to_vec());
        db.load_font_data(LATO_BOLD.to_vec());
        db.set_sans_serif_family(FONT_FAMILY);
        Arc::new(db)
    })
    .clone()
}

pub(super) fn options() -> usvg::Options<'static> {
    usvg::Options {
        font_family: FONT_FAMILY.to_owned(),
        fontdb: fontdb(),
        image_href_resolver: ImageHrefResolver {
            resolve_data: Box::new(|_, _, _| None),
            resolve_string: Box::new(|_, _| None),
        },
        ..Default::default()
    }
}

/// Which annotations [`draw`] draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    /// Arrows, boxes, highlights and callouts. Reports bad colours and
    /// unusable hotspot geometry, for every annotation.
    Shapes,
    /// Hotspot markers only, reporting nothing ([`Layer::Shapes`] did).
    Hotspots,
}

/// Draws the `layer` annotations of `annotations`, in order, onto
/// `pixmap`. Geometry is in full-screenshot pixels; `origin` is where the
/// pixmap's top-left sits in them. Hotspots are numbered among themselves.
pub fn draw(
    annotations: &[Annotation],
    origin: Point,
    pixmap: &mut PixmapMut<'_>,
    layer: Layer,
    warnings: &mut Vec<RenderWarning>,
) {
    let mut ignored = Vec::new();
    let warnings = match layer {
        Layer::Shapes => warnings,
        Layer::Hotspots => &mut ignored,
    };
    let view = View {
        origin,
        width: f64::from(pixmap.width()),
        height: f64::from(pixmap.height()),
    };
    let mut body = String::new();
    let mut hotspots = 0;
    for (n, annotation) in annotations.iter().enumerate() {
        let style = &annotation.style;
        for value in [&style.color, &style.fill].into_iter().flatten() {
            if !is_valid_color(value) {
                warnings.push(RenderWarning::InvalidColor {
                    id: annotation.id,
                    value: value.clone(),
                });
            }
        }
        let result = match &annotation.shape {
            Shape::Hotspot(geometry) => {
                hotspots += 1;
                match finite_rect(&geometry.rect) {
                    Ok(_) if layer == Layer::Hotspots => {
                        hotspot(hotspots, geometry, style, view, &mut body)
                    }
                    Ok(_) => Ok(()),
                    Err(e) => Err(e),
                }
            }
            _ if layer == Layer::Hotspots => Ok(()),
            Shape::Arrow(geometry) => arrow(geometry, style, &mut body),
            Shape::Box(rect) => boxed(rect, style, &mut body),
            Shape::Highlight(rect) => highlight(rect, style, &mut body),
            Shape::Callout(geometry) => {
                callout(n, geometry, style, &mut body, annotation.id, warnings)
            }
            Shape::Blur(_) | Shape::Crop(_) => Ok(()),
        };
        if let Err(reason) = result {
            warnings.push(RenderWarning::AnnotationSkipped {
                id: annotation.id,
                reason,
            });
        }
    }
    if body.is_empty() {
        return;
    }
    let (w, h) = (pixmap.width(), pixmap.height());
    let svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}"><g transform="translate({} {})">{body}</g></svg>"#,
        num(-origin.x),
        num(-origin.y),
    );
    match usvg::Tree::from_str(&svg, &options()) {
        Ok(tree) => resvg::render(&tree, Transform::identity(), pixmap),
        Err(e) => warnings.push(RenderWarning::Annotations(e.to_string())),
    }
}

type Drawn = Result<(), String>;

/// The pixmap's area in full-screenshot pixels.
#[derive(Debug, Clone, Copy)]
struct View {
    origin: Point,
    width: f64,
    height: f64,
}

fn arrow(g: &ArrowGeometry, style: &AnnotationStyle, out: &mut String) -> Drawn {
    let (from, to) = (finite_point(g.from)?, finite_point(g.to)?);
    let (dx, dy) = (to.x - from.x, to.y - from.y);
    let len = dx.hypot(dy);
    if len < 1.0 {
        return Err("arrow is shorter than a pixel".into());
    }
    let sw = stroke_width(style, DEFAULT_STROKE);
    let color = color(style.color.as_deref(), DEFAULT_COLOR);
    let head = (sw * 4.0).max(10.0).min(len);
    let (ux, uy) = (dx / len, dy / len);
    let base = Point {
        x: to.x - ux * head,
        y: to.y - uy * head,
    };
    // Stop the shaft inside the head so the round cap doesn't poke out.
    let shaft = Point {
        x: to.x - ux * head * 0.6,
        y: to.y - uy * head * 0.6,
    };
    let half = head * 0.45;
    let (px, py) = (-uy * half, ux * half);
    let _ = write!(
        out,
        r#"<g{}><line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{color}" stroke-width="{}" stroke-linecap="round"/><polygon points="{},{} {},{} {},{}" fill="{color}"/></g>"#,
        opacity_attr(style, None),
        num(from.x),
        num(from.y),
        num(shaft.x),
        num(shaft.y),
        num(sw),
        num(to.x),
        num(to.y),
        num(base.x + px),
        num(base.y + py),
        num(base.x - px),
        num(base.y - py),
    );
    Ok(())
}

fn boxed(rect: &Rect, style: &AnnotationStyle, out: &mut String) -> Drawn {
    let r = finite_rect(rect)?;
    let sw = stroke_width(style, DEFAULT_STROKE);
    let fill = match style.fill.as_deref() {
        Some(fill) => color(Some(fill), "none"),
        None => "none".into(),
    };
    let _ = write!(
        out,
        r#"<rect{} x="{}" y="{}" width="{}" height="{}" rx="{}" fill="{fill}" stroke="{}" stroke-width="{}"/>"#,
        opacity_attr(style, None),
        num(r.x),
        num(r.y),
        num(r.width),
        num(r.height),
        num(sw),
        color(style.color.as_deref(), DEFAULT_COLOR),
        num(sw),
    );
    Ok(())
}

fn highlight(rect: &Rect, style: &AnnotationStyle, out: &mut String) -> Drawn {
    let r = finite_rect(rect)?;
    let fill = color(
        style.fill.as_deref().or(style.color.as_deref()),
        DEFAULT_HIGHLIGHT,
    );
    let _ = write!(
        out,
        r#"<rect{} x="{}" y="{}" width="{}" height="{}" fill="{fill}"/>"#,
        opacity_attr(style, Some(DEFAULT_HIGHLIGHT_OPACITY)),
        num(r.x),
        num(r.y),
        num(r.width),
        num(r.height),
    );
    Ok(())
}

fn callout(
    n: usize,
    g: &CalloutGeometry,
    style: &AnnotationStyle,
    out: &mut String,
    id: Ulid,
    warnings: &mut Vec<RenderWarning>,
) -> Drawn {
    let r = finite_rect(&g.rect)?;
    let sw = stroke_width(style, DEFAULT_CALLOUT_STROKE);
    let stroke = color(style.color.as_deref(), DEFAULT_COLOR);
    let fill = color(style.fill.as_deref(), DEFAULT_CALLOUT_FILL);
    let font_size = match style.font_size {
        Some(size) if size.is_finite() && size > 0.0 => size.min(512.0),
        _ => DEFAULT_FONT_SIZE,
    };
    let _ = write!(out, "<g{}>", opacity_attr(style, None));
    if let Some(anchor) = g.anchor {
        let anchor = finite_point(anchor)?;
        // From the nearest point of the box edge, so it reads as a pointer.
        let edge = Point {
            x: anchor.x.clamp(r.x, r.x + r.width),
            y: anchor.y.clamp(r.y, r.y + r.height),
        };
        let _ = write!(
            out,
            r#"<line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{stroke}" stroke-width="{}" stroke-linecap="round"/><circle cx="{}" cy="{}" r="{}" fill="{stroke}"/>"#,
            num(edge.x),
            num(edge.y),
            num(anchor.x),
            num(anchor.y),
            num(sw),
            num(anchor.x),
            num(anchor.y),
            num(sw * 1.5 + 1.0),
        );
    }
    let _ = write!(
        out,
        r#"<rect x="{}" y="{}" width="{}" height="{}" rx="{}" fill="{fill}" stroke="{stroke}" stroke-width="{}"/>"#,
        num(r.x),
        num(r.y),
        num(r.width),
        num(r.height),
        num(sw * 2.0),
        num(sw),
    );
    let pad = (font_size * 0.5).max(sw + 2.0);
    let lines = wrap(&g.text, font_size, r.width - 2.0 * pad);
    let line_height = font_size * LINE_HEIGHT;
    if lines.len() as f64 * line_height > r.height - 2.0 * pad + 0.5 {
        warnings.push(RenderWarning::CalloutOverflow { id });
    }
    if !lines.is_empty() {
        let _ = write!(
            out,
            r#"<clipPath id="callout{n}"><rect x="{}" y="{}" width="{}" height="{}"/></clipPath><text clip-path="url(#callout{n})" font-family="{FONT_FAMILY}" font-size="{}" fill="{CALLOUT_TEXT}">"#,
            num(r.x),
            num(r.y),
            num(r.width),
            num(r.height),
            num(font_size),
        );
        for (i, line) in lines.iter().enumerate() {
            let _ = write!(
                out,
                r#"<tspan x="{}" y="{}">{}</tspan>"#,
                num(r.x + pad),
                num(r.y + pad + font_size * 0.9 + i as f64 * line_height),
                escape(line),
            );
        }
        out.push_str("</text>");
    }
    out.push_str("</g>");
    Ok(())
}

/// Hotspot `number`: outline, tint and numbered badge. Nothing is drawn
/// for a hotspot entirely outside `view`, so its badge can't show up at the
/// edge of an unrelated view (it keeps its number in the legend).
fn hotspot(
    number: u32,
    g: &HotspotGeometry,
    style: &AnnotationStyle,
    view: View,
    out: &mut String,
) -> Drawn {
    let r = finite_rect(&g.rect)?;
    let (left, top) = (view.origin.x, view.origin.y);
    let (right, bottom) = (left + view.width, top + view.height);
    if r.x >= right || r.y >= bottom || r.x + r.width <= left || r.y + r.height <= top {
        return Ok(());
    }
    let color = color(style.color.as_deref(), DEFAULT_HOTSPOT_COLOR);
    // No larger than the hotspot, unless that would be unreadable.
    let radius = (view.width * HOTSPOT_BADGE_RATIO)
        .clamp(HOTSPOT_BADGE_MIN, HOTSPOT_BADGE_MAX)
        .min((r.width.min(r.height) / 2.0).max(HOTSPOT_BADGE_MIN));
    let sw = stroke_width(style, (radius / 4.0).max(2.0));
    let ring = (radius / 6.0).max(1.5);
    // Inside the top-left corner, and kept whole inside the view.
    let reach = radius + ring;
    let inside = |v: f64, lo: f64, hi: f64| {
        if hi - lo < 2.0 * reach {
            (lo + hi) / 2.0
        } else {
            v.clamp(lo + reach, hi - reach)
        }
    };
    let inset = reach + sw / 2.0;
    let (cx, cy) = (
        inside(r.x + inset, left, right),
        inside(r.y + inset, top, bottom),
    );
    let dashes = if g.title.trim().is_empty() {
        format!(r#" stroke-dasharray="{} {}""#, num(sw * 3.0), num(sw * 2.0))
    } else {
        String::new()
    };
    let label = number.to_string();
    // Two digits and more get a smaller size to stay inside the badge.
    let font_size = radius * if label.len() > 1 { 0.95 } else { 1.2 };
    let _ = write!(
        out,
        r#"<g{}><rect x="{}" y="{}" width="{}" height="{}" rx="{}" fill="{color}" fill-opacity="{}" stroke="{color}" stroke-width="{}"{dashes}/>"#,
        opacity_attr(style, None),
        num(r.x),
        num(r.y),
        num(r.width),
        num(r.height),
        num(sw),
        num(HOTSPOT_TINT_OPACITY),
        num(sw),
    );
    let _ = write!(
        out,
        r#"<circle cx="{}" cy="{}" r="{}" fill="{color}" stroke="{HOTSPOT_BADGE_TEXT}" stroke-width="{}"/><text x="{}" y="{}" text-anchor="middle" font-family="{FONT_FAMILY}" font-weight="bold" font-size="{}" fill="{HOTSPOT_BADGE_TEXT}">{label}</text></g>"#,
        num(cx),
        num(cy),
        num(radius),
        num(ring),
        num(cx),
        // Digits are about 0.72 em tall in Lato: centre them on the badge.
        num(cy + font_size * 0.36),
        num(font_size),
    );
    Ok(())
}

/// Greedy word wrap into lines no wider than `max_width`; a word wider
/// than that gets a line of its own. Newlines in `text` start new lines.
pub(super) fn wrap(text: &str, font_size: f64, max_width: f64) -> Vec<String> {
    let mut lines = Vec::new();
    for paragraph in text.lines() {
        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            let candidate = if line.is_empty() {
                word.to_owned()
            } else {
                format!("{line} {word}")
            };
            if line.is_empty() || measure(&candidate, font_size) <= max_width {
                line = candidate;
            } else {
                lines.push(std::mem::replace(&mut line, word.to_owned()));
            }
        }
        if !line.is_empty() {
            lines.push(line);
        }
    }
    lines
}

/// Advance width of `text` in the bundled font, in pixels.
pub(super) fn measure(text: &str, font_size: f64) -> f64 {
    let svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"><text x="0" y="{}" font-family="{FONT_FAMILY}" font-size="{}">{}</text></svg>"#,
        num(font_size),
        num(font_size),
        escape(text),
    );
    let Ok(tree) = usvg::Tree::from_str(&svg, &options()) else {
        return 0.0;
    };
    tree.root()
        .children()
        .iter()
        .find_map(|node| match node {
            Node::Text(text) => Some(f64::from(text.bounding_box().width())),
            _ => None,
        })
        .unwrap_or(0.0)
}

fn finite_point(p: Point) -> Result<Point, String> {
    if p.x.is_finite() && p.y.is_finite() {
        Ok(p)
    } else {
        Err("non-finite coordinates".into())
    }
}

fn finite_rect(r: &Rect) -> Result<Rect, String> {
    let ok = [r.x, r.y, r.width, r.height].iter().all(|v| v.is_finite());
    if !ok {
        return Err("non-finite coordinates".into());
    }
    if r.width <= 0.0 || r.height <= 0.0 {
        return Err("empty rectangle".into());
    }
    Ok(*r)
}

fn stroke_width(style: &AnnotationStyle, default: f64) -> f64 {
    match style.stroke_width {
        Some(w) if w.is_finite() && w > 0.0 => w.min(256.0),
        _ => default,
    }
}

/// ` opacity="…"` from the style, else `default`, else nothing.
fn opacity_attr(style: &AnnotationStyle, default: Option<f64>) -> String {
    match style.opacity.filter(|o| o.is_finite()).or(default) {
        Some(opacity) => format!(r#" opacity="{}""#, num(opacity.clamp(0.0, 1.0))),
        None => String::new(),
    }
}

/// `value` if [`is_valid_color`], else `default`. Nothing else reaches the
/// SVG.
fn color(value: Option<&str>, default: &str) -> String {
    match value.map(str::trim) {
        Some(v) if is_valid_color(v) => v.to_owned(),
        _ => default.to_owned(),
    }
}

/// A hex colour (`#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa`), a CSS named
/// colour or `transparent` (case-insensitive, surrounding spaces ignored).
pub fn is_valid_color(value: &str) -> bool {
    let v = value.trim();
    match v.strip_prefix('#') {
        Some(hex) => {
            [3, 4, 6, 8].contains(&hex.len()) && hex.chars().all(|c| c.is_ascii_hexdigit())
        }
        None => {
            let lower = v.to_ascii_lowercase();
            lower == "transparent" || CSS_COLOR_NAMES.binary_search(&lower.as_str()).is_ok()
        }
    }
}

/// The 148 CSS Color Module Level 4 named colours, sorted.
const CSS_COLOR_NAMES: [&str; 148] = [
    "aliceblue",
    "antiquewhite",
    "aqua",
    "aquamarine",
    "azure",
    "beige",
    "bisque",
    "black",
    "blanchedalmond",
    "blue",
    "blueviolet",
    "brown",
    "burlywood",
    "cadetblue",
    "chartreuse",
    "chocolate",
    "coral",
    "cornflowerblue",
    "cornsilk",
    "crimson",
    "cyan",
    "darkblue",
    "darkcyan",
    "darkgoldenrod",
    "darkgray",
    "darkgreen",
    "darkgrey",
    "darkkhaki",
    "darkmagenta",
    "darkolivegreen",
    "darkorange",
    "darkorchid",
    "darkred",
    "darksalmon",
    "darkseagreen",
    "darkslateblue",
    "darkslategray",
    "darkslategrey",
    "darkturquoise",
    "darkviolet",
    "deeppink",
    "deepskyblue",
    "dimgray",
    "dimgrey",
    "dodgerblue",
    "firebrick",
    "floralwhite",
    "forestgreen",
    "fuchsia",
    "gainsboro",
    "ghostwhite",
    "gold",
    "goldenrod",
    "gray",
    "green",
    "greenyellow",
    "grey",
    "honeydew",
    "hotpink",
    "indianred",
    "indigo",
    "ivory",
    "khaki",
    "lavender",
    "lavenderblush",
    "lawngreen",
    "lemonchiffon",
    "lightblue",
    "lightcoral",
    "lightcyan",
    "lightgoldenrodyellow",
    "lightgray",
    "lightgreen",
    "lightgrey",
    "lightpink",
    "lightsalmon",
    "lightseagreen",
    "lightskyblue",
    "lightslategray",
    "lightslategrey",
    "lightsteelblue",
    "lightyellow",
    "lime",
    "limegreen",
    "linen",
    "magenta",
    "maroon",
    "mediumaquamarine",
    "mediumblue",
    "mediumorchid",
    "mediumpurple",
    "mediumseagreen",
    "mediumslateblue",
    "mediumspringgreen",
    "mediumturquoise",
    "mediumvioletred",
    "midnightblue",
    "mintcream",
    "mistyrose",
    "moccasin",
    "navajowhite",
    "navy",
    "oldlace",
    "olive",
    "olivedrab",
    "orange",
    "orangered",
    "orchid",
    "palegoldenrod",
    "palegreen",
    "paleturquoise",
    "palevioletred",
    "papayawhip",
    "peachpuff",
    "peru",
    "pink",
    "plum",
    "powderblue",
    "purple",
    "rebeccapurple",
    "red",
    "rosybrown",
    "royalblue",
    "saddlebrown",
    "salmon",
    "sandybrown",
    "seagreen",
    "seashell",
    "sienna",
    "silver",
    "skyblue",
    "slateblue",
    "slategray",
    "slategrey",
    "snow",
    "springgreen",
    "steelblue",
    "tan",
    "teal",
    "thistle",
    "tomato",
    "turquoise",
    "violet",
    "wheat",
    "white",
    "whitesmoke",
    "yellow",
    "yellowgreen",
];

/// A coordinate for SVG: finite, rounded to 1/1000 px.
pub(super) fn num(v: f64) -> String {
    let v = (v * 1000.0).round() / 1000.0;
    if v == 0.0 {
        "0".into()
    } else {
        format!("{v}")
    }
}

/// Escapes text for XML content and attributes, dropping characters XML
/// cannot contain.
pub(super) fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\t' | '\n' | '\r' => out.push(' '),
            c if (c as u32) < 0x20 || matches!(c, '\u{FFFE}' | '\u{FFFF}') => {}
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_are_validated() {
        assert_eq!(color(Some("#FF3b30"), "x"), "#FF3b30");
        assert_eq!(color(Some(" red "), "x"), "red");
        assert_eq!(color(Some("RebeccaPurple"), "x"), "RebeccaPurple");
        assert_eq!(color(Some("transparent"), "x"), "transparent");
        assert_eq!(color(Some("bleu"), "x"), "x");
        assert_eq!(color(Some("currentColor"), "x"), "x");
        assert_eq!(color(Some("inherit"), "x"), "x");
        assert_eq!(color(Some("#12345"), "x"), "x");
        assert_eq!(color(Some("red\"/><image href=\"x"), "x"), "x");
        assert_eq!(color(Some("url(#a)"), "x"), "x");
        assert_eq!(color(None, "x"), "x");
    }

    #[test]
    fn colour_names_are_sorted() {
        assert!(CSS_COLOR_NAMES.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn escapes_markup() {
        assert_eq!(escape("a<b>&\"'\u{1}\n"), "a&lt;b&gt;&amp;&quot;&apos; ");
    }

    #[test]
    fn bundled_font_measures_text() {
        let short = measure("Save", 16.0);
        let long = measure("Save the file", 16.0);
        assert!(short > 10.0 && short < 60.0, "{short}");
        assert!(long > short * 2.0, "{long} vs {short}");
        assert!((measure("Save", 32.0) - short * 2.0).abs() < 1.0);
    }

    #[test]
    fn wraps_words() {
        let one = measure("Click", 16.0);
        let lines = wrap("Click here to continue\nnow", 16.0, one * 2.5);
        assert_eq!(lines, ["Click here", "to continue", "now"]);
        assert_eq!(wrap("   ", 16.0, 100.0), Vec::<String>::new());
        assert_eq!(
            wrap("Supercalifragilistic", 16.0, 5.0),
            ["Supercalifragilistic"]
        );
    }

    #[test]
    fn numbers_are_compact() {
        assert_eq!(num(-0.0), "0");
        assert_eq!(num(12.0), "12");
        assert_eq!(num(1.23456), "1.235");
    }
}
