//! The derivations this platform runs, as the schemas specification lists them: text from text
//! files and PDFs, and thumbnails of images and of PDFs' first pages. PDFs go through poppler's
//! command-line tools, and images through `GdkPixbuf`.

use std::fs::File;
use std::io::{Read, Seek, Write};
use std::path::Path;
use std::process::{Command, Stdio};

use roxmltree::Document;

use zib_core::schema;

/// The longest side of a thumbnail.
const THUMBNAIL: i32 = 512;

const DC: &str = "http://purl.org/dc/elements/1.1/";
const DCTERMS: &str = "http://purl.org/dc/terms/";
const MAX_PDF_METADATA: u64 = 1024 * 1024;
const PRISM: &str = "http://prismstandard.org/namespaces/basic/";
const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";

/// Reads an embedded byline and publication date, when available.
pub(crate) fn pdf_source(bytes: &[u8]) -> Option<(Vec<String>, Option<String>)> {
    let mut file = tempfile::tempfile().ok()?;
    file.write_all(bytes).ok()?;
    let xmp = pdf_info(&mut file, "-meta").unwrap_or_default();
    let (mut authors, published_on) = xmp_source(&xmp).unwrap_or_default();
    if authors.is_empty()
        && let Some(info) = pdf_info(&mut file, "-custom")
        && let Some(author) = info.lines().find_map(|line| line.strip_prefix("Author:"))
        && let Some(author) = author_name(author)
    {
        authors.push(author);
    }
    Some((authors, published_on))
}

fn pdf_info(file: &mut File, mode: &str) -> Option<String> {
    file.rewind().ok()?;
    let mut child = Command::new("pdfinfo")
        .args([mode, "-enc", "UTF-8", "-"])
        .stdin(file.try_clone().ok()?)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut output = Vec::new();
    let read = child.stdout.take().unwrap().take(MAX_PDF_METADATA + 1).read_to_end(&mut output);
    let complete = read.is_ok_and(|len| len as u64 <= MAX_PDF_METADATA);
    if !complete {
        let _ = child.kill();
    }
    if !child.wait().ok()?.success() || !complete {
        return None;
    }
    String::from_utf8(output).ok()
}

fn xmp_source(xmp: &str) -> Option<(Vec<String>, Option<String>)> {
    let doc = Document::parse(xmp).ok()?;
    let authors = doc
        .descendants()
        .filter(|node| node.has_tag_name((DC, "creator")))
        .flat_map(|creator| creator.children())
        .flat_map(|array| array.children())
        .filter(|node| node.has_tag_name((RDF, "li")))
        .filter_map(|item| item.text().and_then(author_name))
        .take(64)
        .collect();
    let published_on = doc
        .descendants()
        .filter(|node| node.has_tag_name((RDF, "Description")))
        .find_map(|description| {
            let attributes = description.attributes().filter_map(|attr| {
                publication_property(attr.namespace()?, attr.name()).then_some(attr.value())
            });
            let elements = description.children().filter_map(|child| {
                let tag = child.tag_name();
                if publication_property(tag.namespace()?, tag.name()) { child.text() } else { None }
            });
            attributes.chain(elements).find_map(|value| {
                let date = value.trim().split('T').next()?;
                schema::is_date(date).then(|| date.to_owned())
            })
        });
    Some((authors, published_on))
}

fn publication_property(namespace: &str, name: &str) -> bool {
    (namespace == DCTERMS && name == "issued")
        || (namespace.starts_with(PRISM) && name == "publicationDate")
}

fn author_name(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty() && value.len() <= 1024).then(|| value.to_owned())
}

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
    fn reads_ordered_authors_and_the_date_in_xmp() {
        let xmp = r#"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
<rdf:Description xmlns:dc="http://purl.org/dc/elements/1.1/"
    xmlns:prism="http://prismstandard.org/namespaces/basic/2.0/">
  <dc:creator><rdf:Seq>
    <rdf:li>Jamie &amp; Co.</rdf:li>
    <rdf:li>Alex Example</rdf:li>
  </rdf:Seq></dc:creator>
  <prism:publicationDate>2024-02-29T23:45:00-08:00</prism:publicationDate>
</rdf:Description></rdf:RDF>"#;
        assert_eq!(
            xmp_source(xmp),
            Some((vec!["Jamie & Co.".into(), "Alex Example".into()], Some("2024-02-29".into())))
        );
    }

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
