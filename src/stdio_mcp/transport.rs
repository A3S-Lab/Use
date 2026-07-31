use std::future::Future;
use std::io;
use std::sync::Arc;

use futures_util::StreamExt;
use rmcp::service::{RxJsonRpcMessage, ServiceRole, TxJsonRpcMessage};
use rmcp::transport::async_rw::JsonRpcMessageCodec;
use rmcp::transport::Transport;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::sync::Mutex;
use tokio_util::codec::FramedRead;

const MAX_STDIO_MCP_FRAME_BYTES: usize = 4 * 1024 * 1024;

pub(super) type BoxedStdioReader = Box<dyn AsyncRead + Send + Unpin>;
pub(super) type BoxedStdioWriter = Box<dyn AsyncWrite + Send + Unpin>;

/// RMCP's default async-reader codec is intentionally unbounded. Package
/// processes are untrusted, so the compatibility host uses the same standard
/// newline-delimited MCP framing with an explicit frame ceiling in both
/// directions.
pub(super) struct BoundedStdioTransport<Role>
where
    Role: ServiceRole,
{
    reader: FramedRead<BoxedStdioReader, JsonRpcMessageCodec<RxJsonRpcMessage<Role>>>,
    writer: Arc<Mutex<Option<BoxedStdioWriter>>>,
}

impl<Role> BoundedStdioTransport<Role>
where
    Role: ServiceRole,
{
    pub(super) fn new(reader: BoxedStdioReader, writer: BoxedStdioWriter) -> Self {
        Self {
            reader: FramedRead::new(
                reader,
                JsonRpcMessageCodec::new_with_max_length(MAX_STDIO_MCP_FRAME_BYTES),
            ),
            writer: Arc::new(Mutex::new(Some(writer))),
        }
    }
}

impl<Role> Transport<Role> for BoundedStdioTransport<Role>
where
    Role: ServiceRole,
{
    type Error = io::Error;

    fn send(
        &mut self,
        item: TxJsonRpcMessage<Role>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send + 'static {
        let writer = Arc::clone(&self.writer);
        async move {
            let mut frame = serde_json::to_vec(&item)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
            if frame.len() > MAX_STDIO_MCP_FRAME_BYTES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "outbound stdio MCP frame exceeds the bounded contract",
                ));
            }
            frame.push(b'\n');
            let mut writer = writer.lock().await;
            let writer = writer.as_mut().ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotConnected, "stdio MCP transport is closed")
            })?;
            writer.write_all(&frame).await?;
            writer.flush().await
        }
    }

    async fn receive(&mut self) -> Option<RxJsonRpcMessage<Role>> {
        self.reader.next().await.and_then(Result::ok)
    }

    fn close(&mut self) -> impl Future<Output = Result<(), Self::Error>> + Send {
        let writer = Arc::clone(&self.writer);
        async move {
            let mut writer = writer.lock().await.take();
            if let Some(writer) = writer.as_mut() {
                writer.shutdown().await?;
            }
            Ok(())
        }
    }
}
