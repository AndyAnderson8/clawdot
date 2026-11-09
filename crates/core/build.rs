//! Prepares the embedded portal page (REQ-CFG-043): splices the theme into the page and writes both the
//! result and a gzip copy to `OUT_DIR`. Compression happens here, at build time, so the running program
//! needs no compression code at all; it only serves one of the two byte strings.

use flate2::{Compression, write::GzEncoder};
use std::io::Write;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=src/ui.html");
    println!("cargo:rerun-if-changed=src/ui-theme.css");
    println!("cargo:rerun-if-changed=build.rs");
    let page = std::fs::read_to_string("src/ui.html")?;
    let theme = std::fs::read_to_string("src/ui-theme.css")?;
    if !page.contains("/*@theme*/") {
        return Err("src/ui.html has no /*@theme*/ marker".into());
    }
    let page = page.replacen("/*@theme*/", &theme, 1);
    let out = PathBuf::from(std::env::var("OUT_DIR")?);
    std::fs::write(out.join("ui-page.html"), &page)?;
    let mut gz = GzEncoder::new(Vec::new(), Compression::best());
    gz.write_all(page.as_bytes())?;
    std::fs::write(out.join("ui-page.html.gz"), gz.finish()?)?;
    Ok(())
}
