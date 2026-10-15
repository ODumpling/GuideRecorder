# Bundled fonts

Used by `guiderecorder_core::export` to draw callout text (Regular) and to typeset
PDFs (all four, with Typst's own bundled fonts from `typst-assets` as a
fallback for symbols such as ⌘). Only these fonts are loaded (never system
fonts), so output is the same on every machine.

| File | Font | Version | Licence | SHA-256 |
|---|---|---|---|---|
| `Lato-Regular.ttf` | Lato Regular, by Łukasz Dziedzic (tyPoland) | 2.015 (2015-08-06) | SIL Open Font License 1.1, see `Lato-OFL.txt` | `d636e4683231f931eda222d588e944d082bfd3bdba02f928bee461c0f185b251` |
| `Lato-Bold.ttf` | Lato Bold, by Łukasz Dziedzic (tyPoland) | 2.015 (2015-08-06) | SIL Open Font License 1.1, see `Lato-OFL.txt` | `8a0aace75d33794eece4b28187bfc1df0bbd2888b5d8a56e01788c8d65d16be1` |
| `Lato-Italic.ttf` | Lato Italic, by Łukasz Dziedzic (tyPoland) | 2.015 (2015-08-06) | SIL Open Font License 1.1, see `Lato-OFL.txt` | `e399c44efe1387100531d26c7e4800c5d12251b890d6654a3098c7c679cb1786` |
| `Lato-BoldItalic.ttf` | Lato Bold Italic, by Łukasz Dziedzic (tyPoland) | 2.015 (2015-08-06) | SIL Open Font License 1.1, see `Lato-OFL.txt` | `62c1b7f0d2e74b45960154c3520efc337b553db0961bfdc950d5618334596cc8` |

Source: <https://github.com/google/fonts/tree/main/ofl/lato>. The fonts are
bundled unmodified, under their own name ("Lato" is a Reserved Font Name, so a
modified version would have to be renamed).
