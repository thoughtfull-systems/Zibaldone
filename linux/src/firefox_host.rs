//! Firefox's native messaging transport for the space picker and the clip importer.

use std::io::{self, Read, Write};

use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use zib_core::channel::Dirs;
use zib_core::instance::{Client, Reply, Request, SpaceInfo};
use zib_core::web_clip::{Asset, Clip, Imported, MAX_IMAGE_BYTES, MAX_IMAGES, MAX_PDF_BYTES, Pdf};
use zib_core::{Channel, Id};

use zib_linux::CHANNEL;

const MAX_REQUEST_BYTES: usize = 48 * 1024 * 1024;
const MAX_REPLY_BYTES: usize = 1024 * 1024;

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case", deny_unknown_fields)]
enum BrowserRequest {
    Spaces,
    ImportWebClip { clip: Clip, images: Vec<EncodedAsset> },
    ImportWebPdf { data: String, name: String, note: Id, space: Id, url: String },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EncodedAsset {
    note: Id,
    data: String,
}

#[derive(Serialize)]
#[serde(untagged)]
enum BrowserReply {
    Spaces { ok: bool, spaces: Vec<SpaceInfo> },
    Imported { ok: bool, clip: Imported },
    Error { ok: bool, error: String },
}

pub async fn run(start: impl FnOnce() -> io::Result<()>) -> Result<(), String> {
    let reply = match exchange(start).await {
        Ok(reply) => reply,
        Err(error) => BrowserReply::Error { ok: false, error },
    };
    let mut bytes = serde_json::to_vec(&reply).map_err(|e| e.to_string())?;
    if bytes.len() > MAX_REPLY_BYTES {
        bytes = br#"{"ok":false,"error":"Zib returned too much data"}"#.to_vec();
    }
    let len = u32::try_from(bytes.len()).map_err(|e| e.to_string())?;
    let mut stdout = io::stdout().lock();
    stdout.write_all(&len.to_ne_bytes()).map_err(|e| e.to_string())?;
    stdout.write_all(&bytes).map_err(|e| e.to_string())?;
    stdout.flush().map_err(|e| e.to_string())
}

async fn exchange(start: impl FnOnce() -> io::Result<()>) -> Result<BrowserReply, String> {
    if CHANNEL == Channel::Development && CHANNEL.root().is_none() {
        return Err("Set ZIB_DEV_ROOT before starting Firefox with the development host".into());
    }
    let request = read_request(&mut io::stdin().lock())?;
    let dirs = Dirs::for_channel(CHANNEL).map_err(|e| e.to_string())?;
    let mut client = Client::connect(&dirs, start).await.map_err(|e| e.to_string())?;
    match client.request(request).await.map_err(|e| e.to_string())? {
        Reply::Result { clip: Some(clip), .. } => Ok(BrowserReply::Imported { ok: true, clip }),
        Reply::Result { spaces, .. } => Ok(BrowserReply::Spaces { ok: true, spaces }),
        Reply::Error { reason, .. } => Err(reason),
        Reply::Hello { .. } => Err("Zib returned an unexpected reply".into()),
    }
}

fn read_request(reader: &mut impl Read) -> Result<Request, String> {
    let mut len = [0; 4];
    reader.read_exact(&mut len).map_err(|e| e.to_string())?;
    let len = u32::from_ne_bytes(len) as usize;
    if len > MAX_REQUEST_BYTES {
        return Err("Web clip is too large".into());
    }
    let mut bytes = vec![0; len];
    reader.read_exact(&mut bytes).map_err(|e| e.to_string())?;
    let message = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    let id = 1;
    match message {
        BrowserRequest::Spaces => Ok(Request::Spaces { id }),
        BrowserRequest::ImportWebClip { clip, images } => {
            if images.len() > MAX_IMAGES {
                return Err("Web clip has too many images".into());
            }
            let images = images
                .into_iter()
                .map(|image| {
                    if image.data.len() > MAX_IMAGE_BYTES.div_ceil(3) * 4 {
                        return Err("Web clip image is too large");
                    }
                    let bytes = STANDARD.decode(image.data).map_err(|_| "Invalid image data")?;
                    Ok(Asset { note: image.note, bytes })
                })
                .collect::<Result<Vec<_>, _>>()?;
            clip.validate(&images).map_err(|e| e.to_string())?;
            Ok(Request::ImportWebClip { id, clip, images })
        }
        BrowserRequest::ImportWebPdf { data, name, note, space, url } => {
            if data.len() > MAX_PDF_BYTES.div_ceil(3) * 4 {
                return Err("PDF exceeds 32 MiB".into());
            }
            let bytes = STANDARD.decode(data).map_err(|_| "Invalid PDF data")?;
            let pdf = Pdf { bytes, name, note, space, url };
            pdf.validate().map_err(|e| e.to_string())?;
            Ok(Request::ImportWebPdf { id, pdf })
        }
    }
}
