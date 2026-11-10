//! Imports articles with their images, and PDF files, from Firefox.

use std::io::Cursor;

use image::{ImageFormat, ImageReader, Limits};
use zib_core::schema::{self, Value};
use zib_core::space::{FileHash, Note};
use zib_core::web_clip::{Asset, Clip, Imported, Pdf};
use zib_core::{Space, store::Store};

use super::{Error, Id, Result, State, UiEvent};

impl State {
    pub(crate) async fn import_web_clip(&self, clip: Clip, images: Vec<Asset>) -> Result<Imported> {
        clip.validate(&images)?;
        let (images, info) = tokio::task::spawn_blocking(move || {
            let info = images
                .iter()
                .map(|image| {
                    let media = image_media_type(&image.bytes)?;
                    Ok((image.note, FileHash::of(&image.bytes), image.bytes.len() as u64, media))
                })
                .collect::<Result<Vec<_>>>()?;
            Ok::<_, Error>((images, info))
        })
        .await
        .map_err(|error| Error::Protocol(error.to_string()))??;

        let mut store = self.store.lock().await;
        let mut duplicate = None;
        let mut image_in_use = false;
        for &id in store.config.spaces.keys() {
            let space = self.space(id).await?;
            let space = space.lock().await;
            if space.tombstone(clip.note).is_some() {
                return Err(Error::NotFound(format!("article {} was deleted", clip.note)));
            }
            if let Ok(existing) = space.note(clip.note) {
                if existing.type_name != schema::ARTICLE {
                    return Err(Error::NoteChanged(clip.note));
                }
                duplicate = Some(id);
            }
            image_in_use |= images.iter().any(|image| {
                space.note(image.note).is_ok() || space.tombstone(image.note).is_some()
            });
        }
        let result = Imported {
            space: duplicate.unwrap_or(clip.space),
            note: clip.note,
            images: images.iter().map(|image| image.note).collect(),
        };
        if duplicate.is_some() {
            return Ok(result);
        }
        if image_in_use {
            return Err(Error::Protocol("a web clip image identifier is already in use".into()));
        }

        let space = self.space(clip.space).await?;
        let mut space = space.lock().await;
        let staged = clip.stage(&mut space, &info)?;
        let blobs = self.blobs(clip.space);
        for image in &images {
            blobs.add_bytes(&image.bytes)?;
        }
        self.save_web_import(&mut store, &space, &staged, &result)?;
        Ok(result)
    }

    pub(crate) async fn import_web_pdf(&self, pdf: Pdf) -> Result<Imported> {
        pdf.validate()?;
        let mut result = Imported { space: pdf.space, note: pdf.note, images: Vec::new() };
        {
            let mut store = self.store.lock().await;
            for &id in store.config.spaces.keys() {
                let space = self.space(id).await?;
                let space = space.lock().await;
                if space.tombstone(pdf.note).is_some() {
                    return Err(Error::NotFound(format!("PDF {} was deleted", pdf.note)));
                }
                if let Ok(note) = space.note(pdf.note) {
                    if note.type_name != schema::FILE {
                        return Err(Error::NoteChanged(pdf.note));
                    }
                    result.space = id;
                    return Ok(result);
                }
            }
            let space = self.space(pdf.space).await?;
            let mut space = space.lock().await;
            let hash = self.blobs(pdf.space).add_bytes(&pdf.bytes)?;
            let staged = pdf.stage(&mut space, hash)?;
            self.save_web_import(&mut store, &space, &staged, &result)?;
        }
        let _ = self.derive(result.space).await;
        Ok(result)
    }

    fn save_web_import(
        &self,
        store: &mut Store,
        space: &Space,
        staged: &Space,
        result: &Imported,
    ) -> Result<()> {
        let Some(updates) = staged.updates_since(&space.version())? else {
            return Err(Error::Protocol("web clip made no changes".into()));
        };
        store.record_peer(staged)?;
        store.save_space(staged)?;
        // Apply the same operations to the live document, keeping open editors connected to it.
        space.doc().import(&updates)?;
        self.reindex_note(result.space, space, result.note);
        for &image in &result.images {
            self.reindex_note(result.space, space, image);
        }
        self.changed_here(result.space);
        self.send_ui(UiEvent::Changed(result.space));
        Ok(())
    }

    pub(crate) async fn article_fields(
        &self,
        space: Id,
        note: &Note,
    ) -> Vec<(&'static str, Value)> {
        if note.type_name != schema::ARTICLE {
            return Vec::new();
        }
        let Ok(space) = self.space(space).await else { return Vec::new() };
        let space = space.lock().await;
        ["authors", "captured-at", "published-on", "url"]
            .into_iter()
            .filter_map(|name| note.field(&space, name).map(|value| (name, value)))
            .collect()
    }
}

fn image_media_type(bytes: &[u8]) -> Result<&'static str> {
    let invalid = || Error::Protocol("invalid or unsupported web clip image".into());
    let format = image::guess_format(bytes).map_err(|_| invalid())?;
    if !matches!(
        format,
        ImageFormat::Gif | ImageFormat::Jpeg | ImageFormat::Png | ImageFormat::WebP
    ) {
        return Err(invalid());
    }
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    reader.decode().map_err(|_| invalid())?;
    Ok(format.to_mime_type())
}

#[cfg(test)]
mod tests {
    use zib_core::body;
    use zib_core::channel::Dirs;
    use zib_core::web_clip::Part;

    use super::*;

    #[tokio::test]
    async fn imports_article_and_images_durably_and_reuses_the_ids_after_restart() {
        let temp = tempfile::tempdir().unwrap();
        let dirs = Dirs::under(temp.path());
        let state = State::for_test(dirs.clone()).await.unwrap();
        let space = state.spaces().await[0].0;
        let note = Id::generate();
        let picture = Id::generate();
        let clip = Clip {
            space,
            note,
            title: "A field guide: Moss, trees & birds (vol. 2)!".into(),
            url: "https://example.org/article".into(),
            captured_at: 1_700_000_000_000,
            published_on: Some("2023-02-28".into()),
            authors: vec!["Test Author".into()],
            body: vec![
                Part::Markdown("An unusual article.\n\n".into()),
                Part::Image { image: 0, alt: "A | picture".into() },
            ],
        };
        let mut png = Cursor::new(Vec::new());
        image::RgbImage::from_pixel(2, 2, image::Rgb([255, 0, 0]))
            .write_to(&mut png, ImageFormat::Png)
            .unwrap();
        let images = vec![Asset { note: picture, bytes: png.into_inner() }];
        let invalid = vec![Asset { note: picture, bytes: vec![1, 2, 3] }];
        assert!(state.import_web_clip(clip.clone(), invalid).await.is_err());
        assert!(state.notes(space).await.unwrap().is_empty());

        let result = state.import_web_clip(clip.clone(), images.clone()).await.unwrap();
        assert_eq!(result, Imported { space, note, images: vec![picture] });
        drop(state);

        let state = State::for_test(dirs).await.unwrap();
        assert_eq!(state.import_web_clip(clip.clone(), images.clone()).await.unwrap(), result);
        assert_eq!(state.notes(space).await.unwrap().len(), 2);
        let article = state.note(space, note).await.unwrap();
        assert_eq!(article.type_name, schema::ARTICLE);
        assert_eq!(article.body.lines().next(), Some(format!("# {}", clip.title).as_str()));
        assert_eq!(body::title(&article.body, &mut |_| None).plain, clip.title);
        assert_eq!(body::wikilinks(&article.body)[0].target, picture);
        let fields = state.article_fields(space, &article).await;
        assert!(fields.contains(&("url", Value::Text(clip.url.clone()))));
        assert!(fields.contains(&("published-on", Value::Date("2023-02-28".into()))));
        assert_eq!(state.search("unusual", 20)[0].1.id, note);
        assert_eq!(state.search("Test Author", 20)[0].1.id, note);
        let (hash, media) = state.note_file(space, picture, "image").await.unwrap().unwrap();
        assert_eq!(media, "image/png");
        assert_eq!(state.blobs(space).read(hash).unwrap(), images[0].bytes);

        let destination = state.create_space("Moved").await.unwrap();
        state.move_note(space, destination, note).await.unwrap();
        let retried = state.import_web_clip(clip.clone(), images.clone()).await.unwrap();
        assert_eq!(retried, Imported { space: destination, ..result });
        state.delete_note(destination, note).await.unwrap();
        assert!(state.import_web_clip(clip, images).await.is_err());
    }

    #[tokio::test]
    async fn imports_an_original_pdf_as_a_durable_file_note() {
        let temp = tempfile::tempdir().unwrap();
        let dirs = Dirs::under(temp.path());
        let state = State::for_test(dirs.clone()).await.unwrap();
        let space = state.spaces().await[0].0;
        let original = state.create_note(space, "Words to print").await.unwrap();
        let bytes = state.print(&[original], false).await.unwrap();
        let note = Id::generate();
        let pdf = Pdf {
            bytes,
            name: "Saved PDF.pdf".into(),
            note,
            space,
            url: "https://example.org/document".into(),
        };
        let invalid = Pdf { bytes: b"an HTML login page".to_vec(), ..pdf.clone() };
        assert!(state.import_web_pdf(invalid).await.is_err());
        assert!(state.note(space, note).await.is_err());

        let imported = state.import_web_pdf(pdf.clone()).await.unwrap();
        assert_eq!(imported, Imported { space, note, images: Vec::new() });
        drop(state);
        let state = State::for_test(dirs).await.unwrap();
        assert_eq!(state.import_web_pdf(pdf.clone()).await.unwrap(), imported);
        let file = state.note(space, note).await.unwrap();
        assert_eq!(file.type_name, schema::FILE);
        assert_eq!(file.body, "Saved PDF.pdf\n\n[Source](<https://example.org/document>)\n");
        let (hash, media) = state.note_file(space, note, "file").await.unwrap().unwrap();
        assert_eq!(media, "application/pdf");
        assert_eq!(state.blobs(space).read(hash).unwrap(), pdf.bytes);
    }
}
