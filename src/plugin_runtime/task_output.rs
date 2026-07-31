use a3s_runtime::contract::{RuntimeLogQuery, RuntimeLogStream};
use a3s_use_core::{UseError, UseResult};
use serde::Serialize;
use tokio::io::{AsyncWrite, AsyncWriteExt};

use super::client::{runtime_error, PluginRuntimeClient};
use super::model::{runtime_contract_error, RuntimeSurfacePlan};

pub const MAX_IN_MEMORY_TASK_OUTPUT_BYTES: u64 = 16 * 1024 * 1024;

const LOG_QUERY_CHUNKS: u32 = 64;
const MAX_LOG_QUERY_ROUNDS: usize = 1_024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeTaskOutputSummary {
    pub bytes_written: u64,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeTaskStreamingExecution {
    pub observation: a3s_runtime::contract::RuntimeObservation,
    pub exit_code: i32,
    pub stdout: RuntimeTaskOutputSummary,
    pub stderr: RuntimeTaskOutputSummary,
}

impl PluginRuntimeClient {
    pub(super) async fn capture_log_stream<Writer>(
        &self,
        plan: &RuntimeSurfacePlan,
        stream: RuntimeLogStream,
        max_bytes: u64,
        writer: &mut Writer,
    ) -> UseResult<RuntimeTaskOutputSummary>
    where
        Writer: AsyncWrite + Unpin + Send + ?Sized,
    {
        let mut cursor = None;
        let mut last_sequence = None;
        let mut bytes_written = 0_u64;
        for _ in 0..MAX_LOG_QUERY_ROUNDS {
            let query = RuntimeLogQuery {
                schema: RuntimeLogQuery::SCHEMA.to_string(),
                unit_id: plan.spec().unit_id.clone(),
                generation: plan.spec().generation,
                cursor: cursor.clone(),
                limit: LOG_QUERY_CHUNKS,
                stream: Some(stream),
            };
            query.validate().map_err(runtime_contract_error)?;
            let chunks = self
                .client
                .logs(&query)
                .await
                .map_err(|error| runtime_error("read Runtime Task output", error))?;
            if chunks.is_empty() {
                return Ok(RuntimeTaskOutputSummary {
                    bytes_written,
                    truncated: false,
                });
            }
            let previous_cursor = cursor.clone();
            for chunk in chunks {
                chunk.validate().map_err(runtime_contract_error)?;
                if chunk.stream != stream
                    || last_sequence.is_some_and(|sequence| chunk.sequence <= sequence)
                {
                    return Err(runtime_contract_error(
                        "Runtime Task log chunks are out of order or crossed streams.",
                    ));
                }
                last_sequence = Some(chunk.sequence);
                cursor = Some(chunk.cursor);
                let chunk_bytes = u64::try_from(chunk.data.len()).map_err(|_| {
                    runtime_contract_error("Runtime Task log chunk size does not fit this host.")
                })?;
                let remaining = max_bytes.saturating_sub(bytes_written);
                if chunk_bytes > remaining {
                    let end = utf8_prefix_len(&chunk.data, remaining)?;
                    write_output(writer, stream, &chunk.data.as_bytes()[..end]).await?;
                    bytes_written = bytes_written
                        .checked_add(u64::try_from(end).map_err(|_| {
                            runtime_contract_error(
                                "Runtime Task output size does not fit its contract.",
                            )
                        })?)
                        .ok_or_else(|| {
                            runtime_contract_error("Runtime Task output size overflowed.")
                        })?;
                    return Ok(RuntimeTaskOutputSummary {
                        bytes_written,
                        truncated: true,
                    });
                }
                write_output(writer, stream, chunk.data.as_bytes()).await?;
                bytes_written = bytes_written.checked_add(chunk_bytes).ok_or_else(|| {
                    runtime_contract_error("Runtime Task output size overflowed.")
                })?;
            }
            if cursor == previous_cursor {
                return Err(runtime_contract_error(
                    "Runtime Task log cursor did not advance.",
                ));
            }
        }
        Err(runtime_contract_error(
            "Runtime Task log pagination exceeded its bounded round count.",
        ))
    }
}

pub(super) async fn flush_output<Writer>(
    writer: &mut Writer,
    stream: RuntimeLogStream,
) -> UseResult<()>
where
    Writer: AsyncWrite + Unpin + Send + ?Sized,
{
    writer.flush().await.map_err(|error| {
        output_error(
            stream,
            format!(
                "Failed to flush streamed Runtime Task {}: {error}",
                stream_name(stream)
            ),
            error.kind(),
        )
    })
}

async fn write_output<Writer>(
    writer: &mut Writer,
    stream: RuntimeLogStream,
    bytes: &[u8],
) -> UseResult<()>
where
    Writer: AsyncWrite + Unpin + Send + ?Sized,
{
    writer.write_all(bytes).await.map_err(|error| {
        output_error(
            stream,
            format!(
                "Failed to write streamed Runtime Task {}: {error}",
                stream_name(stream)
            ),
            error.kind(),
        )
    })
}

fn utf8_prefix_len(value: &str, max_bytes: u64) -> UseResult<usize> {
    let mut end = usize::try_from(max_bytes)
        .map_err(|_| runtime_contract_error("Runtime Task capture bound does not fit this host."))?
        .min(value.len());
    while !value.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    Ok(end)
}

fn output_error(
    stream: RuntimeLogStream,
    message: impl Into<String>,
    kind: std::io::ErrorKind,
) -> UseError {
    UseError::new("use.plugin.runtime.output_write_failed", message)
        .with_detail("stream", stream_name(stream))
        .with_detail("ioKind", format!("{kind:?}"))
}

fn stream_name(stream: RuntimeLogStream) -> &'static str {
    match stream {
        RuntimeLogStream::Stdout => "stdout",
        RuntimeLogStream::Stderr => "stderr",
    }
}
