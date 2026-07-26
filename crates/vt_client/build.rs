//! Emit the string table as JSON for the HUD page.
//!
//! The fleet's rule is that **Rust is the only thing that parses the CSV**:
//! the format has quoted fields, embedded commas and newlines, doubled quotes
//! and CRLF, and two implementations of one format is the shape of a bug this
//! fleet has already shipped once. So the page never sees `en.csv` — it
//! fetches what this wrote.
//!
//! The output lands in the source assets directory rather than `OUT_DIR`
//! because Trunk's `copy-dir` is what ships it to the browser; it is
//! gitignored, and `rerun-if-changed` is scoped to the CSV so writing it does
//! not retrigger the build.
//!
//! If it is somehow missing at runtime the page is not blank: every
//! `data-i18n` element keeps its authored English as the text a fetch would
//! replace, so the worst case is an untranslated HUD rather than an empty one.

use std::path::Path;

fn main() {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets the manifest dir");
    let csv_path = Path::new(&manifest).join("assets/strings/en.csv");
    println!("cargo:rerun-if-changed={}", csv_path.display());
    println!("cargo:rerun-if-changed=build.rs");

    let source = std::fs::read_to_string(&csv_path)
        .unwrap_or_else(|e| panic!("reading {}: {e}", csv_path.display()));
    let table = vellum_strings::Table::parse(vellum_strings::Locale::ENGLISH, &source)
        .unwrap_or_else(|errors| {
            for error in &errors {
                println!("cargo:warning=en.csv: {error}");
            }
            panic!("{} problems in en.csv", errors.len());
        });

    let out = Path::new(&manifest).join("assets/strings/en.json");
    std::fs::write(&out, table.to_json())
        .unwrap_or_else(|e| panic!("writing {}: {e}", out.display()));
}
