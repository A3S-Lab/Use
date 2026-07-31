use std::io;
use std::pin::Pin;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::task::{Context, Poll};

use a3s_runtime::contract::{NetworkMode, RuntimeLogStream};
use a3s_use_core::{PluginSurfaceKind, ToolWorkloadContract};
use tempfile::TempDir;
use tokio::fs;
use tokio::io::AsyncWrite;

use super::test_support::*;
use super::*;

#[test]
fn streaming_task_output_types_are_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}

    assert_send_sync::<RuntimeTaskOutputSummary>();
    assert_send_sync::<RuntimeTaskStreamingExecution>();
}

#[tokio::test]
async fn task_binding_invokes_native_argv_and_captures_separate_output_streams() {
    let descriptor = task_descriptor();
    let plan = plan_tool_task_release(
        context(PluginSurfaceKind::Tool, "convert"),
        &task_surface(),
        &descriptor,
        artifact(&descriptor.artifact.digest, &descriptor.artifact.media_type),
        RuntimeTaskInvocation::new("invoke-01", vec!["--input".into(), "paper.pdf".into()])
            .unwrap(),
        policy(),
        NetworkMode::None,
    )
    .unwrap();
    let capabilities = capabilities(&plan);
    let provider = evidence(&plan, &capabilities);
    let runtime = Arc::new(FakeRuntime::new(capabilities, true).with_logs(vec![
        log_chunk(RuntimeLogStream::Stdout, 1, "stdout-1", "{\"ok\":true}\n"),
        log_chunk(RuntimeLogStream::Stderr, 1, "stderr-1", "diagnostic\n"),
    ]));
    let client = PluginRuntimeClient::new(runtime.clone());
    let binding = client.prepare_task(&plan, &provider).await.unwrap();
    let result = client
        .invoke_task(&plan, &binding, "invoke-request-01", Some(9_999_999))
        .await
        .unwrap();

    assert_eq!(runtime.apply_count.load(Ordering::SeqCst), 1);
    assert_eq!(runtime.remove_count.load(Ordering::SeqCst), 1);
    assert_eq!(result.exit_code, 0);
    assert_eq!(result.stdout, "{\"ok\":true}\n");
    assert_eq!(result.stderr, "diagnostic\n");
    assert!(!result.truncated);
    assert_eq!(
        plan.spec().process.args,
        vec!["--input".to_string(), "paper.pdf".to_string()]
    );
}

#[tokio::test]
async fn large_capture_uses_host_owned_files_without_an_in_memory_apply() {
    let mut descriptor = task_descriptor();
    let ToolWorkloadContract::Task {
        max_stdout_bytes,
        max_stderr_bytes,
        ..
    } = &mut descriptor.workload
    else {
        panic!("fixture should be a Task");
    };
    *max_stdout_bytes = 16 * 1024 * 1024 + 1;
    *max_stderr_bytes = 16 * 1024 * 1024 + 1;
    let plan = plan_tool_task_release(
        context(PluginSurfaceKind::Tool, "convert"),
        &task_surface(),
        &descriptor,
        artifact(&descriptor.artifact.digest, &descriptor.artifact.media_type),
        RuntimeTaskInvocation::new("invoke-01", Vec::new()).unwrap(),
        policy(),
        NetworkMode::None,
    )
    .unwrap();
    let capabilities = capabilities(&plan);
    let provider = evidence(&plan, &capabilities);
    let runtime = Arc::new(FakeRuntime::new(capabilities, true).with_logs(vec![
        log_chunk(RuntimeLogStream::Stdout, 1, "stdout-1", "large-output\n"),
        log_chunk(RuntimeLogStream::Stderr, 1, "stderr-1", "diagnostic\n"),
    ]));
    let client = PluginRuntimeClient::new(runtime.clone());
    let binding = client.prepare_task(&plan, &provider).await.unwrap();

    let error = client
        .invoke_task(&plan, &binding, "in-memory-01", Some(9_999_999))
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.plugin.runtime.capture_unsupported");
    assert_eq!(runtime.apply_count.load(Ordering::SeqCst), 0);

    let temporary = TempDir::new().unwrap();
    let stdout_path = temporary.path().join("stdout.log");
    let stderr_path = temporary.path().join("stderr.log");
    let mut stdout = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&stdout_path)
        .await
        .unwrap();
    let mut stderr = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&stderr_path)
        .await
        .unwrap();
    let result = client
        .invoke_task_streaming(
            &plan,
            &binding,
            "streaming-01",
            Some(9_999_999),
            &mut stdout,
            &mut stderr,
        )
        .await
        .unwrap();
    drop(stdout);
    drop(stderr);

    assert_eq!(runtime.apply_count.load(Ordering::SeqCst), 1);
    assert_eq!(runtime.remove_count.load(Ordering::SeqCst), 1);
    assert_eq!(result.stdout.bytes_written, 13);
    assert_eq!(result.stderr.bytes_written, 11);
    assert!(!result.stdout.truncated);
    assert!(!result.stderr.truncated);
    assert_eq!(
        fs::read_to_string(stdout_path).await.unwrap(),
        "large-output\n"
    );
    assert_eq!(
        fs::read_to_string(stderr_path).await.unwrap(),
        "diagnostic\n"
    );
}

#[tokio::test]
async fn streaming_capture_truncates_only_at_a_utf8_boundary() {
    let mut descriptor = task_descriptor();
    let ToolWorkloadContract::Task {
        max_stdout_bytes, ..
    } = &mut descriptor.workload
    else {
        panic!("fixture should be a Task");
    };
    *max_stdout_bytes = 5;
    let plan = plan_tool_task_release(
        context(PluginSurfaceKind::Tool, "convert"),
        &task_surface(),
        &descriptor,
        artifact(&descriptor.artifact.digest, &descriptor.artifact.media_type),
        RuntimeTaskInvocation::new("invoke-01", Vec::new()).unwrap(),
        policy(),
        NetworkMode::None,
    )
    .unwrap();
    let capabilities = capabilities(&plan);
    let provider = evidence(&plan, &capabilities);
    let runtime = Arc::new(
        FakeRuntime::new(capabilities, true).with_logs(vec![log_chunk(
            RuntimeLogStream::Stdout,
            1,
            "stdout-1",
            "ééé",
        )]),
    );
    let client = PluginRuntimeClient::new(runtime);
    let binding = client.prepare_task(&plan, &provider).await.unwrap();
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();

    let result = client
        .invoke_task_streaming(
            &plan,
            &binding,
            "streaming-01",
            Some(9_999_999),
            &mut stdout,
            &mut stderr,
        )
        .await
        .unwrap();

    assert_eq!(String::from_utf8(stdout).unwrap(), "éé");
    assert_eq!(result.stdout.bytes_written, 4);
    assert!(result.stdout.truncated);
    assert_eq!(result.stderr.bytes_written, 0);
    assert!(!result.stderr.truncated);
}

#[tokio::test]
async fn streaming_sink_failure_still_removes_the_exact_task_unit() {
    let descriptor = task_descriptor();
    let plan = plan_tool_task_release(
        context(PluginSurfaceKind::Tool, "convert"),
        &task_surface(),
        &descriptor,
        artifact(&descriptor.artifact.digest, &descriptor.artifact.media_type),
        RuntimeTaskInvocation::new("invoke-01", Vec::new()).unwrap(),
        policy(),
        NetworkMode::None,
    )
    .unwrap();
    let capabilities = capabilities(&plan);
    let provider = evidence(&plan, &capabilities);
    let runtime = Arc::new(
        FakeRuntime::new(capabilities, true).with_logs(vec![log_chunk(
            RuntimeLogStream::Stdout,
            1,
            "stdout-1",
            "output",
        )]),
    );
    let client = PluginRuntimeClient::new(runtime.clone());
    let binding = client.prepare_task(&plan, &provider).await.unwrap();
    let mut stdout = FailingWriter;
    let mut stderr = Vec::new();

    let error = client
        .invoke_task_streaming(
            &plan,
            &binding,
            "streaming-01",
            Some(9_999_999),
            &mut stdout,
            &mut stderr,
        )
        .await
        .unwrap_err();

    assert_eq!(error.code, "use.plugin.runtime.output_write_failed");
    assert_eq!(runtime.remove_count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn ambiguous_task_apply_failure_attempts_exact_cleanup() {
    let descriptor = task_descriptor();
    let plan = plan_tool_task_release(
        context(PluginSurfaceKind::Tool, "convert"),
        &task_surface(),
        &descriptor,
        artifact(&descriptor.artifact.digest, &descriptor.artifact.media_type),
        RuntimeTaskInvocation::new("invoke-01", Vec::new()).unwrap(),
        policy(),
        NetworkMode::None,
    )
    .unwrap();
    let capabilities = capabilities(&plan);
    let provider = evidence(&plan, &capabilities);
    let runtime = Arc::new(FakeRuntime::new(capabilities, true).with_apply_failure());
    let client = PluginRuntimeClient::new(runtime.clone());
    let binding = client.prepare_task(&plan, &provider).await.unwrap();

    let error = client
        .invoke_task(&plan, &binding, "invoke-01", Some(9_999_999))
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.plugin.runtime.operation_failed");
    assert_eq!(runtime.stop_count.load(Ordering::SeqCst), 1);
    assert_eq!(runtime.remove_count.load(Ordering::SeqCst), 1);
}

struct FailingWriter;

impl AsyncWrite for FailingWriter {
    fn poll_write(
        self: Pin<&mut Self>,
        _context: &mut Context<'_>,
        _buffer: &[u8],
    ) -> Poll<io::Result<usize>> {
        Poll::Ready(Err(io::Error::other("injected output failure")))
    }

    fn poll_flush(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}
