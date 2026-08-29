//! Content-Length framed message transport, the base-protocol layer of the
//! Language Server Protocol.
//!
//! <https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#baseProtocol>

use anyhow::{Context as _, Result, bail};
use smol::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};

const CONTENT_LENGTH: &str = "content-length:";

/// Reads framed messages from a language server's output stream.
pub struct MessageReader<R> {
    reader: BufReader<R>,
    line: String,
}

impl<R: AsyncRead + Unpin> MessageReader<R> {
    pub fn new(reader: R) -> Self {
        Self {
            reader: BufReader::new(reader),
            line: String::new(),
        }
    }

    /// Read the next message payload. Returns `None` on a clean end of
    /// stream (the server exited).
    pub async fn read(&mut self) -> Result<Option<Vec<u8>>> {
        let mut content_length: Option<usize> = None;
        loop {
            self.line.clear();
            let read = self.reader.read_line(&mut self.line).await?;
            if read == 0 {
                if content_length.is_none() {
                    return Ok(None);
                }
                bail!("unexpected end of stream inside a message header");
            }

            let line = self.line.trim_end();
            if line.is_empty() {
                break;
            }
            if let Some(value) = line.to_ascii_lowercase().strip_prefix(CONTENT_LENGTH) {
                content_length = Some(value.trim().parse().context("invalid Content-Length")?);
            }
        }

        let content_length = content_length.context("message without a Content-Length header")?;
        let mut payload = vec![0; content_length];
        self.reader.read_exact(&mut payload).await?;
        Ok(Some(payload))
    }
}

/// Writes framed messages to a language server's input stream.
pub struct MessageWriter<W> {
    writer: W,
}

impl<W: AsyncWrite + Unpin> MessageWriter<W> {
    pub fn new(writer: W) -> Self {
        Self { writer }
    }

    pub async fn write(&mut self, payload: &[u8]) -> Result<()> {
        let header = format!("Content-Length: {}\r\n\r\n", payload.len());
        self.writer.write_all(header.as_bytes()).await?;
        self.writer.write_all(payload).await?;
        self.writer.flush().await?;
        Ok(())
    }
}
