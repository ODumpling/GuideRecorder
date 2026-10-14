//! `guidesya` CLI. See design scratchpad #441; commands land in #343 and #353.

fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("--version") | Some("-V") => {
            println!("guidesya {}", env!("CARGO_PKG_VERSION"));
        }
        _ => {
            eprintln!("usage: guidesya --version  (list/export/reindex/validate coming soon)");
            std::process::exit(2);
        }
    }
}
