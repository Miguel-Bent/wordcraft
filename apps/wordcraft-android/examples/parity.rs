//! Parity check: runs a fixed script on the engine's built-in documents and prints a SHA-256
//! fingerprint of every page render and every export. The same program is built for the PC and
//! for Android; identical output on both means the engine produces the same results on both.
//!
//! Host:    cargo run -p wordcraft-android --example parity --release > host.txt
//! Android: see scripts/parity-android.ps1

use serde_json::json;
use sha2::{Digest, Sha256};
use wordcraft_engine::{Session, io, sample};
use wordcraft_render::{RenderOptions, render_page};

fn fingerprint(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// PDFs stamp the creation time from the clock (`D:AAAAMMDDHHMMSS`), so two runs never match
/// byte for byte. Blank the digits (same length, so sizes still compare) before fingerprinting.
fn without_pdf_dates(bytes: &[u8]) -> Vec<u8> {
    let mut out = bytes.to_vec();
    let mut i = 0;
    while i + 16 <= out.len() {
        if &out[i..i + 2] == b"D:" && out[i + 2..i + 16].iter().all(u8::is_ascii_digit) {
            out[i + 2..i + 16].fill(b'0');
            i += 16;
        } else {
            i += 1;
        }
    }
    out
}

fn main() {
    let documents: [(&str, fn() -> wordcraft_engine::doc::Document); 4] = [
        ("sample", sample::sample_document),
        ("letter", sample::letter),
        ("resume", sample::resume),
        ("report", sample::report),
    ];
    for (name, make) in documents {
        let mut doc = make();
        // Fixed creation and modification dates: exports stamp the clock otherwise, so two runs
        // (and the PC and the phone) would never produce the same bytes.
        doc.core.created = "2024-01-01T00:00:00Z".to_string();
        doc.core.modified = "2024-01-01T00:00:00Z".to_string();
        let mut session = Session::new(doc);
        // The same edits everywhere: insert text, bold it, then undo the last change.
        for (command, params) in [
            ("text.insert", json!({ "text": "Olá, mundo" })),
            ("format.bold", json!({})),
            ("text.insert", json!({ "text": " fim" })),
            ("edit.undo", json!({})),
        ] {
            let outcome = session.run(command, &params);
            println!("{name}\tcommand\t{command}\t{}", if outcome.is_ok() { "ok" } else { "error" });
        }

        let layout = session.export_layout();
        println!("{name}\tpages\t{}", layout.pages.len());
        for (index, page) in layout.pages.iter().enumerate() {
            let rendered = render_page(&session.doc, page, 1.0, &RenderOptions::default());
            println!(
                "{name}\tpage\t{index}\t{}x{}\t{}",
                rendered.width,
                rendered.height,
                fingerprint(&rendered.to_straight())
            );
        }

        for extension in ["docx", "odt", "rtf", "md", "txt", "html", "pdf"] {
            let file = format!("{name}.{extension}");
            match io::save_bytes(&file, &session.doc) {
                Ok(bytes) => {
                    let comparable = if extension == "pdf" { without_pdf_dates(&bytes) } else { bytes.clone() };
                    println!("{name}\texport\t{extension}\t{}\t{}", bytes.len(), fingerprint(&comparable));
                }
                Err(error) => println!("{name}\texport\t{extension}\terror\t{error}"),
            }
        }
    }
}
