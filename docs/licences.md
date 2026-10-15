# Dependency licences

Audit of 2026-10-14 (Solo todo #373). GuideRecorder is MIT-licensed (see `LICENSE`), so only
permissive dependency licences are used; nothing GPL/LGPL/AGPL is linked.

## Rust (`cargo metadata`, ~800 third-party crates)

- MIT / Apache-2.0 (and dual forms): the vast majority.
- Also permissive: BSD-2/3-Clause, ISC, Zlib, 0BSD, Unlicense, CC0-1.0,
  BSL-1.0, Unicode-3.0 (ICU data), CDLA-Permissive-2.0 (webpki root certs).
- `r-efi` is "MIT OR Apache-2.0 OR LGPL-2.1-or-later": used under MIT.
- `aws-lc-sys` (rustls crypto via reqwest): ISC / Apache-2.0 / MIT /
  BSD-3-Clause / OpenSSL-style terms, all permissive.
- MPL-2.0 (weak, file-level copyleft) only via Tauri: `cssparser`,
  `cssparser-macros`, `selectors`, `dtoa-short`, `option-ext`. Fine for an
  MIT app as long as those files are not modified.

## Frontend (`pnpm licenses list --prod`)

129 packages (updated with the TipTap editor): 123 MIT, plus 0BSD (tslib),
Apache-2.0 (class-variance-authority), Apache-2.0 OR MIT (@tauri-apps/api),
ISC (lucide-react), BSD-2-Clause (entities, via markdown-it) and PSF-2.0
(argparse, only used by markdown-it's CLI and not in the app bundle). All
permissive. Direct additions: 13 TipTap 3.32 packages and markdown-it 15
(MIT).

## Bundled assets

- Lato Regular/Bold/Italic/BoldItalic 2.015: SIL OFL 1.1
  (`crates/guiderecorder-core/assets/fonts`, hashes in its README).
- typst-assets fonts (DejaVu Sans Mono, New Computer Modern, Libertinus
  Serif): their own free font licences (Bitstream Vera/DejaVu, GUST/OFL,
  OFL), redistributable; embedded as PDF subsets.

## Keeping it clean

`deny.toml` encodes the policy for `cargo deny check licenses`. Re-run this
audit when adding dependencies; new crates must be permissive.
