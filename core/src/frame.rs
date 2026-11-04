//! Frames of CBOR maps, as the sync specification defines them for every protocol between
//! machines and processes.

use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::{Error, Result};

/// The most a frame holds on `zib/sync` and the instance's socket.
pub const MAX_FRAME: usize = 1 << 30;
/// The most a frame holds on `zib/pair`.
pub const MAX_PAIR_FRAME: usize = 1 << 16;

/// Writes one message as a 4-byte big-endian length and a CBOR map.
pub async fn write<W, T>(writer: &mut W, message: &T, max: usize) -> Result<()>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    let mut bytes = Vec::new();
    ciborium::into_writer(message, &mut bytes).map_err(|e| Error::Protocol(e.to_string()))?;
    if bytes.len() > max {
        return Err(Error::Protocol(format!("a message of {} bytes is too long", bytes.len())));
    }
    let len = u32::try_from(bytes.len()).map_err(|_| Error::Protocol("message too long".into()))?;
    writer.write_all(&len.to_be_bytes()).await?;
    writer.write_all(&bytes).await?;
    writer.flush().await?;
    Ok(())
}

/// Reads one message, or `None` when the stream ends cleanly before a frame starts. A length
/// beyond `max` ends the stream without reading the rest.
pub async fn read<R, T>(reader: &mut R, max: usize) -> Result<Option<T>>
where
    R: AsyncRead + Unpin,
    T: DeserializeOwned,
{
    let mut len = [0u8; 4];
    match reader.read_exact(&mut len).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e.into()),
    }
    let len = u32::from_be_bytes(len) as usize;
    if len > max {
        return Err(Error::Protocol(format!("a frame of {len} bytes is too long")));
    }
    let mut bytes = vec![0u8; len];
    reader.read_exact(&mut bytes).await?;
    ciborium::from_reader(bytes.as_slice()).map(Some).map_err(|e| Error::Protocol(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq, serde::Deserialize, Serialize)]
    struct Message {
        #[serde(rename = "type")]
        kind: String,
        #[serde(with = "serde_bytes")]
        data: Vec<u8>,
    }

    #[derive(Debug, PartialEq, serde::Deserialize)]
    struct Older {
        #[serde(rename = "type")]
        kind: String,
    }

    #[tokio::test]
    async fn round_trips_and_ignores_unknown_keys() {
        let (mut a, mut b) = tokio::io::duplex(1024);
        let message = Message { kind: "hello".into(), data: vec![1, 2, 3] };
        write(&mut a, &message, MAX_FRAME).await.unwrap();
        write(&mut a, &message, MAX_FRAME).await.unwrap();
        drop(a);
        assert_eq!(read::<_, Message>(&mut b, MAX_FRAME).await.unwrap(), Some(message));
        let older: Older = read(&mut b, MAX_FRAME).await.unwrap().unwrap();
        assert_eq!(older.kind, "hello");
        assert_eq!(read::<_, Message>(&mut b, MAX_FRAME).await.unwrap(), None);
    }

    #[tokio::test]
    async fn refuses_long_frames() {
        let (mut a, mut b) = tokio::io::duplex(64);
        a.write_all(&u32::MAX.to_be_bytes()).await.unwrap();
        assert!(read::<_, Older>(&mut b, MAX_PAIR_FRAME).await.is_err());
    }
}
