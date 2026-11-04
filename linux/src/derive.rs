//! The derivations this platform runs, as the schemas specification lists them: text from text
//! files and PDFs, and thumbnails of images and of PDFs' first pages. PDFs go through poppler's
//! command-line tools, and images through `GdkPixbuf`.

use std::path::Path;
use std::process::Command;

/// The longest side of a thumbnail.
const THUMBNAIL: i32 = 512;

/// Runs a derivation on the file at `path`, and returns the blob it makes with its media type, or
/// `None` when this machine can't run it on this file.
pub fn run(derivation: &str, path: &Path, media_type: &str) -> Option<(Vec<u8>, String)> {
    match derivation {
        "text" => text(path, media_type).map(|t| (t.into_bytes(), "text/plain".to_owned())),
        "thumbnail" => thumbnail(path, media_type),
        _ => None,
    }
}

fn text(path: &Path, media_type: &str) -> Option<String> {
    if media_type.starts_with("text/") {
        let bytes = std::fs::read(path).ok()?;
        return Some(String::from_utf8_lossy(&bytes).into_owned());
    }
    if media_type == "application/pdf" {
        let output =
            Command::new("pdftotext").arg("-enc").arg("UTF-8").arg(path).arg("-").output().ok()?;
        let text = String::from_utf8(output.stdout).ok()?;
        // Pages come separated by form feeds, which become blank lines.
        let text = text.replace('\u{c}', "\n\n");
        return (output.status.success() && !text.trim().is_empty()).then_some(text);
    }
    None
}

fn thumbnail(path: &Path, media_type: &str) -> Option<(Vec<u8>, String)> {
    if media_type == "application/pdf" {
        let dir = tempfile_dir()?;
        let prefix = dir.join("page");
        let status = Command::new("pdftoppm")
            .args(["-png", "-singlefile", "-scale-to", &THUMBNAIL.to_string()])
            .arg(path)
            .arg(&prefix)
            .status()
            .ok()?;
        let png = std::fs::read(prefix.with_extension("png")).ok();
        let _ = std::fs::remove_dir_all(&dir);
        return status.success().then_some(png?).map(|png| (png, "image/png".to_owned()));
    }
    if media_type.starts_with("image/") {
        let pixbuf =
            gdk_pixbuf::Pixbuf::from_file_at_scale(path, THUMBNAIL, THUMBNAIL, true).ok()?;
        let (format, media) =
            if pixbuf.has_alpha() { ("png", "image/png") } else { ("jpeg", "image/jpeg") };
        let bytes = pixbuf.save_to_bufferv(format, &[]).ok()?;
        return Some((bytes, media.to_owned()));
    }
    None
}

/// A fresh private directory for a command's output.
fn tempfile_dir() -> Option<std::path::PathBuf> {
    let dir = std::env::temp_dir().join(format!("zibaldone-{}", zib_core::Id::generate()));
    std::fs::create_dir(&dir).ok()?;
    Some(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_text_files() {
        let dir = std::env::temp_dir().join(format!("zib-derive-{}", zib_core::Id::generate()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.txt");
        std::fs::write(&file, "hello").unwrap();
        assert_eq!(
            run("text", &file, "text/plain"),
            Some((b"hello".to_vec(), "text/plain".to_owned()))
        );
        assert_eq!(run("text", &file, "application/zip"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
