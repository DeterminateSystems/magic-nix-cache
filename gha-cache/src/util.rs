//! Utilities.
//!
//! Taken from <https://github.com/zhaofengli/attic>.

use bytes::{Bytes, BytesMut};
use tokio::io::{AsyncRead, AsyncReadExt};

/// Greedily reads from a stream to fill a buffer.
pub async fn read_chunk_async<S: AsyncRead + Unpin + Send>(
    stream: &mut S,
    mut chunk: BytesMut,
) -> std::io::Result<Bytes> {
    while chunk.len() < chunk.capacity() {
        let read = stream.read_buf(&mut chunk).await?;

        if read == 0 {
            break;
        }
    }

    Ok(chunk.freeze())
}
