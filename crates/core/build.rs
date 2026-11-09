//! Prepares the embedded portal page (REQ-CFG-043): copies the committed page to `OUT_DIR` and writes a gzip
//! copy beside it. Compression happens here, at build time, so the running program needs no compression code
//! at all; it only serves one of the two byte strings.
//!
//! `src/ui.html` is generated from the TypeScript sources in `portal/` (see ADR-0016) and committed, so a Rust
//! build never needs Node. CI rebuilds the page and fails if the committed copy is out of date.

use flate2::{Compression, write::GzEncoder};
use std::io::Write;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=src/ui.html");
    println!("cargo:rerun-if-changed=build.rs");
    let page = std::fs::read_to_string("src/ui.html")?;
    if !page.contains("<script>") {
        return Err(
            "src/ui.html looks wrong: it has no inline script (run `npm run build` in portal/)"
                .into(),
        );
    }
    let out = PathBuf::from(std::env::var("OUT_DIR")?);
    std::fs::write(out.join("ui-page.html"), &page)?;
    let mut gz = GzEncoder::new(Vec::new(), Compression::best());
    gz.write_all(page.as_bytes())?;
    std::fs::write(out.join("ui-page.html.gz"), gz.finish()?)?;
    Ok(())
}
