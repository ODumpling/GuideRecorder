// GuideRecorder PDF template. See `export/pdf.rs` for the data contract.
//
// Every piece of guide text arrives as a string in `/guide.json` and is
// placed as a value (never parsed as markup or evaluated), so nothing in a
// guide can run Typst code. Do not pass guide text to `eval`, `read`,
// `json`, `image`, `link` labels or anything else that interprets strings.

#let data = json("/guide.json")
#let labels = data.labels
#let accent = rgb(data.accent)
#let muted = rgb("#5f6368")
#let link-color = rgb("#0563c1")
#let code-fill = luma(242)

// Fonts: Lato for text; Typst's bundled fonts (no system fonts) fill in
// glyphs Lato lacks, such as ⌘ ⇧ ⌥, and set code.
#let text-fonts = ("Lato", "DejaVu Sans Mono", "New Computer Modern Math", "Libertinus Serif")
#let code-fonts = ("DejaVu Sans Mono", "Lato", "New Computer Modern Math")

#let styled(s, bold, italic) = {
  let body = if italic { emph(s) } else { s }
  if bold { strong(body) } else { body }
}

// Markdown inlines: (t: "text" | "code" | "strong" | "em" | "link" | "br").
#let inlines(nodes, bold: false, italic: false) = {
  for n in nodes {
    if n.t == "text" {
      styled(n.v, bold, italic)
    } else if n.t == "code" {
      box(
        fill: code-fill,
        inset: (x: 2.5pt),
        outset: (y: 2.5pt),
        radius: 2pt,
        text(font: code-fonts, size: 0.88em, styled(n.v, bold, false)),
      )
    } else if n.t == "strong" {
      inlines(n.c, bold: true, italic: italic)
    } else if n.t == "em" {
      inlines(n.c, bold: bold, italic: true)
    } else if n.t == "link" {
      link(n.url, text(fill: link-color, underline(inlines(n.c, bold: bold, italic: italic))))
    } else if n.t == "br" {
      linebreak()
    }
  }
}

// Markdown blocks: (p: inlines) or (list: items, start: none | int), where
// each list item is again an array of blocks.
#let blocks(bs) = {
  for b in bs {
    if "p" in b {
      inlines(b.p)
      parbreak()
    } else {
      let items = b.list.map(item => blocks(item))
      if b.start == none { list(..items) } else { enum(start: b.start, ..items) }
    }
  }
}

#let note-colors = (
  tip: (fill: rgb("#e8f5e9"), line: rgb("#2e7d32")),
  warning: (fill: rgb("#fff4e5"), line: rgb("#ed6c02")),
  note: (fill: rgb("#e8f0fe"), line: rgb("#1a73e8")),
)

// The label leads the first paragraph, so the two never part.
#let note(n) = {
  let colors = note-colors.at(n.note)
  let label = text(fill: colors.line, styled(n.label + ":", true, false))
  let (first, rest) = if n.body.len() > 0 and "p" in n.body.at(0) {
    (n.body.at(0).p, n.body.slice(1))
  } else {
    ((), n.body)
  }
  block(
    width: 100%,
    fill: colors.fill,
    stroke: (left: 3pt + colors.line),
    inset: (x: 12pt, y: 9pt),
    radius: (right: 3pt),
    above: 1.2em,
    below: 1.2em,
  )[
    #label #inlines(first)
    #parbreak()
    #blocks(rest)
  ]
}

// The numbered legend of a step's hotspots: (number, color, title, lines);
// title and lines are plain text, color a checked hex colour. The image
// shows the same numbers in the same colours.
#let legend(hotspots) = {
  if hotspots.len() > 0 {
    block(above: 0.9em, below: 1.1em, grid(
      columns: (auto, 1fr),
      column-gutter: 7pt,
      row-gutter: 7pt,
      ..hotspots.map(h => (
        box(
          fill: rgb(h.color),
          radius: 50%,
          width: 1.45em,
          height: 1.45em,
          align(center + horizon, text(fill: white, size: 0.85em, weight: "bold", str(h.number))),
        ),
        block(above: 0pt, {
          if h.title != "" { strong(h.title) }
          for (i, line) in h.lines.enumerate() {
            if i > 0 or h.title != "" { linebreak() }
            line
          }
        }),
      )).flatten(),
    ))
  }
}

// The step's heading and image never split, and stick to the description
// (Typst moves them to the next page with its first lines when it doesn't
// start on this one). The description flows freely, however long it is.
#let step(s) = {
  block(breakable: false, sticky: true, width: 100%, above: 1.8em, below: 0.9em)[
    #heading(level: 2, {
      str(s.number) + ". "
      if s.title.len() == 0 { labels.step + " " + str(s.number) } else { inlines(s.title) }
    })
    #let (w, h) = (s.image.width * 0.75pt, s.image.height * 0.75pt)
    #layout(size => {
      let width = calc.min(w, size.width)
      let height = width * (h / w)
      if height > data.max_image_height * 1pt {
        height = data.max_image_height * 1pt
        width = height * (w / h)
      }
      align(center, block(
        stroke: 0.5pt + luma(210),
        image(s.image.path, width: width, height: height, alt: s.image.alt),
      ))
    })
  ]
  legend(s.hotspots)
  blocks(s.description)
}

#set document(title: data.title)
#set text(font: text-fonts, size: 10.5pt, lang: data.lang, hyphenate: false)
#set par(leading: 0.65em, spacing: 0.9em)
#set list(indent: 0.4em)
#set enum(indent: 0.4em)
#show heading.where(level: 1): set text(size: 16pt, fill: accent)
#show heading.where(level: 1): set block(above: 2em, below: 1em)
#show heading.where(level: 2): set text(size: 12.5pt)
#show heading.where(level: 2): set block(above: 0pt, below: 0.9em)

#set page(
  paper: data.paper,
  margin: (x: 2cm, top: 2.6cm, bottom: 2.4cm),
  header: context if here().page() > 1 {
    set text(size: 8.5pt, fill: muted)
    block(width: 100%, stroke: (bottom: 0.5pt + luma(200)), inset: (bottom: 5pt), data.title)
  },
  footer: context if here().page() > 1 {
    set text(size: 8.5pt, fill: muted)
    align(right, counter(page).display("1 / 1", both: true))
  },
)

// Cover page.
#{
  // The logo is 1.6 cm high, or less if that would be wider than the page.
  if data.logo != none {
    layout(size => {
      let ratio = data.logo.width / data.logo.height
      image(data.logo.path, height: calc.min(1.6cm, size.width / ratio))
    })
  }
  v(1fr)
  block(width: 100%, stroke: (left: 4pt + accent), inset: (left: 16pt, y: 4pt))[
    #text(size: 28pt, weight: "bold", data.title)
    #if data.summary != "" {
      parbreak()
      text(size: 14pt, fill: muted, data.summary)
    }
  ]
  v(1fr)
  if data.date != none {
    text(fill: muted, data.date)
  }
  if data.details != none {
    if data.date != none { linebreak() }
    text(size: 9.5pt, fill: muted, data.details)
  }
  pagebreak()
}

#blocks(data.intro)

#if data.prerequisites.len() > 0 {
  heading(level: 1, labels.prerequisites)
  blocks(data.prerequisites)
}

#for item in data.items {
  if item.kind == "section" {
    heading(level: 1, item.title)
  } else if item.kind == "step" {
    step(item)
  } else if item.kind == "note" {
    note(item)
  }
}
