//! Browser captures and their representation as article, file, and image notes.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::schema::{self, Value};
use crate::space::FileHash;
use crate::{Error, Id, Result, Space, body};

pub const MAX_BODY_BYTES: usize = 1024 * 1024;
pub const MAX_IMAGE_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_IMAGES: usize = 64;
pub const MAX_PDF_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_TOTAL_IMAGE_BYTES: usize = 32 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct Clip {
    pub space: Id,
    pub note: Id,
    pub title: String,
    pub url: String,
    pub captured_at: i64,
    #[serde(default)]
    pub published_on: Option<String>,
    #[serde(default)]
    pub authors: Vec<String>,
    pub body: Vec<Part>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(untagged)]
pub enum Part {
    Markdown(String),
    Image { image: usize, alt: String },
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Asset {
    pub note: Id,
    #[serde(with = "serde_bytes")]
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Pdf {
    #[serde(with = "serde_bytes")]
    pub bytes: Vec<u8>,
    pub name: String,
    pub note: Id,
    pub space: Id,
    pub url: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Imported {
    pub space: Id,
    pub note: Id,
    pub images: Vec<Id>,
}

/// The HTTP(S) address that the browser captured and the desktop can open.
pub fn source_url(text: &str) -> Option<url::Url> {
    let url = url::Url::parse(text).ok()?;
    (matches!(url.scheme(), "http" | "https")
        && url.host().is_some()
        && url.username().is_empty()
        && url.password().is_none())
    .then_some(url)
}

impl Pdf {
    pub fn validate(&self) -> Result<()> {
        if self.bytes.len() > MAX_PDF_BYTES {
            return Err(invalid("PDF exceeds 32 MiB"));
        }
        if !self.bytes.starts_with(b"%PDF-") {
            return Err(invalid("the downloaded file has no PDF header"));
        }
        if !is_v7(self.note)
            || self.name.trim().is_empty()
            || self.name.len() > 4096
            || self.name.chars().any(char::is_control)
            || self.url.len() > 8192
            || source_url(&self.url).is_none()
        {
            return Err(invalid("invalid PDF metadata"));
        }
        Ok(())
    }

    /// Stages an ordinary file note with a link back to the source.
    pub fn stage(&self, space: &mut Space, hash: FileHash) -> Result<Space> {
        let url = source_url(&self.url).ok_or_else(|| invalid("invalid PDF source URL"))?;
        let url = url.as_str().replace('<', "%3C").replace('>', "%3E");
        let body = format!("{}\n\n[Source](<{url}>)\n", self.name.trim());
        let mut staged = space.stage_import()?;
        staged.add_file(hash, self.bytes.len() as u64, "application/pdf")?;
        staged.insert_note(self.note, schema::FILE, &body, &[("file", Value::File(hash))])?;
        staged.commit();
        Ok(staged)
    }
}

impl Clip {
    pub fn validate(&self, images: &[Asset]) -> Result<()> {
        let metadata_ok = !self.title.trim().is_empty()
            && self.title.len() <= 4096
            && self.url.len() <= 8192
            && source_url(&self.url).is_some()
            && self.captured_at >= 0
            && self.published_on.as_deref().is_none_or(schema::is_date)
            && self.authors.len() <= 64
            && self.authors.iter().all(|author| !author.trim().is_empty() && author.len() <= 1024);
        if !metadata_ok {
            return Err(invalid("invalid article metadata"));
        }
        let mut ids = HashSet::from([self.note]);
        if images.len() > MAX_IMAGES
            || !is_v7(self.note)
            || images.iter().any(|a| {
                !is_v7(a.note)
                    || !ids.insert(a.note)
                    || a.bytes.is_empty()
                    || a.bytes.len() > MAX_IMAGE_BYTES
            })
            || images.iter().map(|a| a.bytes.len()).sum::<usize>() > MAX_TOTAL_IMAGE_BYTES
        {
            return Err(invalid("invalid image data or identifiers"));
        }
        let mut used = HashSet::new();
        let mut body_bytes = 0;
        let mut content = false;
        for part in &self.body {
            match part {
                Part::Markdown(text) => {
                    body_bytes += text.len();
                    content |= !text.trim().is_empty();
                }
                Part::Image { image, alt } => {
                    if *image >= images.len() || alt.len() > 4096 {
                        return Err(invalid("invalid image reference"));
                    }
                    used.insert(*image);
                    body_bytes += alt.len() + 32;
                }
            }
        }
        if body_bytes > MAX_BODY_BYTES
            || used.len() != images.len()
            || (!content && used.is_empty())
        {
            return Err(invalid("invalid article body"));
        }
        Ok(())
    }

    /// Builds one commit on a fork. The caller saves the fork before publishing its changes.
    pub fn stage(&self, space: &mut Space, images: &[(Id, FileHash, u64, &str)]) -> Result<Space> {
        let mut staged = space.stage_import()?;
        let title = self.title.split_whitespace().collect::<Vec<_>>().join(" ");
        let mut text = format!("# {title}\n\n");
        for part in &self.body {
            match part {
                Part::Markdown(markdown) => text.push_str(markdown),
                Part::Image { image, alt } => {
                    let &(id, ..) = images.get(*image).ok_or_else(|| invalid("missing image"))?;
                    text.push('!');
                    let label = escape_text(alt);
                    text.push_str(&body::wikilink(
                        id,
                        (!label.is_empty()).then_some(label.as_str()),
                    ));
                }
            }
        }

        let mut fields = vec![
            ("captured-at", Value::Timestamp(self.captured_at)),
            ("url", Value::Text(self.url.clone())),
        ];
        if !self.authors.is_empty() {
            fields.push((
                "authors",
                Value::List(self.authors.iter().cloned().map(Value::Text).collect()),
            ));
        }
        if let Some(date) = &self.published_on {
            fields.push(("published-on", Value::Date(date.clone())));
        }
        for (index, (note, hash, size, media_type)) in images.iter().enumerate() {
            let image_title = format!("Image {} from {title}\n", index + 1);
            staged.add_file(*hash, *size, media_type)?;
            staged.insert_note(
                *note,
                schema::IMAGE,
                &image_title,
                &[("image", Value::File(*hash))],
            )?;
        }
        staged.insert_note(self.note, schema::ARTICLE, &text, &fields)?;
        staged.commit();
        Ok(staged)
    }
}

fn is_v7(id: Id) -> bool {
    let bytes = id.as_bytes();
    bytes[6] >> 4 == 7 && bytes[8] >> 6 == 2
}

/// Escapes plain text for inline Markdown, including Zib's extra punctuation.
fn escape_text(text: &str) -> String {
    let mut escaped = String::new();
    for character in text.split_whitespace().collect::<Vec<_>>().join(" ").chars() {
        if character.is_ascii_punctuation() {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

fn invalid(reason: &str) -> Error {
    Error::Protocol(format!("web clip: {reason}"))
}
