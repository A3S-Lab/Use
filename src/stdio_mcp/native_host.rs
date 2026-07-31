use std::io;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use a3s_use_core::{PlanEnforcementProfile, UseError, UseResult};
use async_trait::async_trait;
#[cfg(windows)]
use process_wrap::tokio::JobObject;
#[cfg(unix)]
use process_wrap::tokio::ProcessGroup;
use process_wrap::tokio::{KillOnDrop, TokioChildWrapper, TokioCommandWrap};
use tokio::process::Command;
use tokio::runtime::Handle;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use super::model::{
    SpawnedStdioMcpSession, StdioMcpHostCapabilities, StdioMcpHostFeature, StdioMcpHostProvider,
    StdioMcpProcessControl, StdioMcpSessionPlan,
};
use super::process_model::{StdioMcpProcessIdentity, StdioMcpProcessObservation};
use super::validation::unix_time_ms;

mod paths;

use paths::validate_native_paths;

const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(20);

/// Production native stdio host without a filesystem, network, process, or
/// resource sandbox.
///
/// This provider is intentionally explicit about its
/// [`PlanEnforcementProfile::NativeUnconfined`] boundary. It is suitable only
/// for an active grant carrying exact user confirmation; the session planner
/// continues to reject unattended use and all secret-bearing permissions.
/// On Unix, cleanup owns the launched process group but cannot prevent a
/// deliberately daemonizing child from escaping it; that requires an actual
/// child-process confinement provider.
#[derive(Debug, Clone)]
pub struct NativeUnconfinedStdioMcpHost {
    capabilities: StdioMcpHostCapabilities,
}

impl NativeUnconfinedStdioMcpHost {
    /// Construct native provider evidence from an immutable host build ID.
    pub fn new(provider_build_id: impl Into<String>) -> UseResult<Self> {
        Ok(Self {
            capabilities: StdioMcpHostCapabilities::new(
                native_provider_id(),
                provider_build_id,
                PlanEnforcementProfile::NativeUnconfined,
                vec![
                    StdioMcpHostFeature::SanitizedEnvironment,
                    StdioMcpHostFeature::OwnedFilesystemRoots,
                    StdioMcpHostFeature::ProcessIdentity,
                    StdioMcpHostFeature::StderrDrain,
                    StdioMcpHostFeature::ProcessTreeCleanup,
                ],
            )?,
        })
    }

    /// Immutable capability evidence returned during both supervisor checks.
    pub fn capability_evidence(&self) -> &StdioMcpHostCapabilities {
        &self.capabilities
    }
}

#[async_trait]
impl StdioMcpHostProvider for NativeUnconfinedStdioMcpHost {
    async fn capabilities(&self) -> UseResult<StdioMcpHostCapabilities> {
        Ok(self.capabilities.clone())
    }

    async fn spawn(&self, plan: &StdioMcpSessionPlan) -> UseResult<SpawnedStdioMcpSession> {
        let runtime = Handle::try_current().map_err(|_| {
            native_error(
                "use.plugin.stdio_mcp.native_runtime_missing",
                "The native stdio MCP host requires an active Tokio runtime.",
            )
        })?;
        if plan.provider() != &self.capabilities {
            return Err(native_error(
                "use.plugin.stdio_mcp.native_provider_mismatch",
                "The native stdio MCP plan is bound to different provider evidence.",
            ));
        }
        self.capabilities
            .validate_for(plan.permission(), plan.grant_authority())?;
        validate_native_paths(plan).await?;

        let mut command = Command::new(plan.executable());
        command
            .args(plan.args())
            .current_dir(plan.package_root())
            .env_clear()
            .envs(plan.non_secret_environment())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut command = TokioCommandWrap::from(command);
        #[cfg(unix)]
        command.wrap(ProcessGroup::leader());
        #[cfg(windows)]
        command.wrap(JobObject);
        command.wrap(KillOnDrop);
        let mut child = command.spawn().map_err(|error| {
            native_io_error(
                "use.plugin.stdio_mcp.native_spawn_failed",
                "The native stdio MCP process group could not be spawned.",
                &error,
            )
        })?;
        let process_id = match child.id() {
            Some(process_id) => process_id,
            None => {
                return abort_spawn(
                    child,
                    None,
                    native_error(
                        "use.plugin.stdio_mcp.native_spawn_failed",
                        "The native stdio MCP process has no OS identity.",
                    ),
                )
                .await
            }
        };
        let reader = match child.stdout().take() {
            Some(reader) => reader,
            None => {
                return abort_spawn(
                    child,
                    Some(process_id),
                    native_error(
                        "use.plugin.stdio_mcp.native_spawn_failed",
                        "The native stdio MCP stdout pipe is unavailable.",
                    ),
                )
                .await
            }
        };
        let writer = match child.stdin().take() {
            Some(writer) => writer,
            None => {
                return abort_spawn(
                    child,
                    Some(process_id),
                    native_error(
                        "use.plugin.stdio_mcp.native_spawn_failed",
                        "The native stdio MCP stdin pipe is unavailable.",
                    ),
                )
                .await
            }
        };
        let stderr = match child.stderr().take() {
            Some(stderr) => stderr,
            None => {
                return abort_spawn(
                    child,
                    Some(process_id),
                    native_error(
                        "use.plugin.stdio_mcp.native_spawn_failed",
                        "The native stdio MCP stderr pipe is unavailable.",
                    ),
                )
                .await
            }
        };
        let started_at_ms = match unix_time_ms() {
            Ok(started_at_ms) => started_at_ms,
            Err(error) => return abort_spawn(child, Some(process_id), error).await,
        };
        let identity =
            match StdioMcpProcessIdentity::new(plan, format!("pid-{process_id}"), started_at_ms) {
                Ok(identity) => identity,
                Err(error) => return abort_spawn(child, Some(process_id), error).await,
            };
        let (state_sender, state) = watch::channel(NativeProcessState::Running);
        let termination = CancellationToken::new();
        let control = Arc::new(NativeProcessControl {
            identity,
            state,
            termination: termination.clone(),
        });
        let session = match SpawnedStdioMcpSession::new(reader, writer, control.clone()) {
            Ok(session) => session,
            Err(error) => return abort_spawn(child, Some(process_id), error).await,
        };

        let stderr_drain = runtime.spawn(async move {
            let mut stderr = stderr;
            tokio::io::copy(&mut stderr, &mut tokio::io::sink()).await
        });
        runtime.spawn(monitor_process_group(
            child,
            process_id,
            state_sender,
            termination,
            stderr_drain,
        ));
        Ok(session)
    }
}

#[derive(Debug, Clone)]
enum NativeProcessState {
    Running,
    Fault(UseError),
    Exited { exit_code: Option<i32> },
}

#[derive(Debug)]
struct NativeProcessControl {
    identity: StdioMcpProcessIdentity,
    state: watch::Receiver<NativeProcessState>,
    termination: CancellationToken,
}

#[async_trait]
impl StdioMcpProcessControl for NativeProcessControl {
    fn identity(&self) -> &StdioMcpProcessIdentity {
        &self.identity
    }

    async fn observe(&self) -> UseResult<StdioMcpProcessObservation> {
        observation(&self.identity, &self.state.borrow().clone())
    }

    async fn wait_for_exit(&self) -> UseResult<StdioMcpProcessObservation> {
        let mut state = self.state.clone();
        loop {
            match state.borrow().clone() {
                NativeProcessState::Running => {}
                NativeProcessState::Fault(error) => return Err(error),
                terminal @ NativeProcessState::Exited { .. } => {
                    return observation(&self.identity, &terminal)
                }
            }
            state.changed().await.map_err(|_| {
                native_error(
                    "use.plugin.stdio_mcp.native_monitor_failed",
                    "The native stdio MCP process monitor ended without terminal evidence.",
                )
            })?;
        }
    }

    fn terminate(&self) {
        self.termination.cancel();
    }
}

impl Drop for NativeProcessControl {
    fn drop(&mut self) {
        self.termination.cancel();
    }
}

async fn monitor_process_group(
    mut child: Box<dyn TokioChildWrapper>,
    process_id: u32,
    state: watch::Sender<NativeProcessState>,
    termination: CancellationToken,
    stderr_drain: tokio::task::JoinHandle<io::Result<u64>>,
) {
    loop {
        let root_exited = match child.inner_mut().try_wait() {
            Ok(Some(_)) => true,
            Ok(None) => {
                state.send_replace(NativeProcessState::Running);
                false
            }
            Err(error) => {
                state.send_replace(NativeProcessState::Fault(native_io_error(
                    "use.plugin.stdio_mcp.native_observe_failed",
                    "The native stdio MCP root process could not be observed.",
                    &error,
                )));
                false
            }
        };
        if root_exited || termination.is_cancelled() {
            match child.start_kill() {
                Ok(()) => break,
                Err(error) if process_group_absent(&error) => break,
                Err(error) => {
                    state.send_replace(NativeProcessState::Fault(native_io_error(
                        "use.plugin.stdio_mcp.native_terminate_failed",
                        "The native stdio MCP process group could not be terminated.",
                        &error,
                    )));
                }
            }
        }
        tokio::time::sleep(PROCESS_POLL_INTERVAL).await;
    }

    let status = loop {
        match Box::into_pin(child.wait()).await {
            Ok(status) => break status,
            Err(error) => {
                state.send_replace(NativeProcessState::Fault(native_io_error(
                    "use.plugin.stdio_mcp.native_wait_failed",
                    "The native stdio MCP process group could not be reaped.",
                    &error,
                )));
                request_group_termination(&mut child);
                tokio::time::sleep(PROCESS_POLL_INTERVAL).await;
            }
        }
    };
    let _ = stderr_drain.await;
    if let Err(error) = wait_for_process_group_absence(process_id).await {
        state.send_replace(NativeProcessState::Fault(error));
        return;
    }
    state.send_replace(NativeProcessState::Exited {
        exit_code: status.code(),
    });
}

fn observation(
    identity: &StdioMcpProcessIdentity,
    state: &NativeProcessState,
) -> UseResult<StdioMcpProcessObservation> {
    let observed_at_ms = unix_time_ms()?;
    match state {
        NativeProcessState::Running => {
            StdioMcpProcessObservation::running(identity.clone(), observed_at_ms)
        }
        NativeProcessState::Fault(error) => Err(error.clone()),
        NativeProcessState::Exited { exit_code } => {
            StdioMcpProcessObservation::exited(identity.clone(), *exit_code, observed_at_ms)
        }
    }
}

async fn abort_spawn<T>(
    mut child: Box<dyn TokioChildWrapper>,
    process_id: Option<u32>,
    mut primary: UseError,
) -> UseResult<T> {
    if let Err(cleanup) = terminate_and_reap(&mut child, process_id).await {
        primary = primary
            .with_detail("cleanupCode", cleanup.code)
            .with_detail("cleanupMessage", cleanup.message);
    }
    Err(primary)
}

async fn terminate_and_reap(
    child: &mut Box<dyn TokioChildWrapper>,
    process_id: Option<u32>,
) -> UseResult<()> {
    match child.start_kill() {
        Ok(()) => {}
        Err(error) if process_group_absent(&error) => {}
        Err(error) => {
            return Err(native_io_error(
                "use.plugin.stdio_mcp.native_terminate_failed",
                "The native stdio MCP process group could not be terminated after spawn failure.",
                &error,
            ))
        }
    }
    child.stdin().take();
    child.stdout().take();
    child.stderr().take();
    Box::into_pin(child.wait()).await.map_err(|error| {
        native_io_error(
            "use.plugin.stdio_mcp.native_wait_failed",
            "The native stdio MCP process group could not be reaped after spawn failure.",
            &error,
        )
    })?;
    if let Some(process_id) = process_id {
        wait_for_process_group_absence(process_id).await?;
    }
    Ok(())
}

fn request_group_termination(child: &mut Box<dyn TokioChildWrapper>) {
    match child.start_kill() {
        Ok(()) => {}
        Err(error) if process_group_absent(&error) => {}
        Err(_) => {}
    }
}

fn process_group_absent(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::InvalidInput || {
        #[cfg(unix)]
        {
            error.raw_os_error() == Some(libc::ESRCH)
        }
        #[cfg(windows)]
        {
            false
        }
    }
}

#[cfg(unix)]
async fn wait_for_process_group_absence(process_id: u32) -> UseResult<()> {
    let process_group = i32::try_from(process_id).map_err(|_| {
        native_error(
            "use.plugin.stdio_mcp.native_identity_invalid",
            "The native stdio MCP process group identity is outside the OS range.",
        )
    })?;
    loop {
        // SAFETY: signal 0 performs a liveness check only; the negated,
        // checked PID is the process group created for this exact child.
        let result = unsafe { libc::kill(-process_group, 0) };
        if result == 0 {
            tokio::time::sleep(PROCESS_POLL_INTERVAL).await;
            continue;
        }
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            return Ok(());
        }
        return Err(native_io_error(
            "use.plugin.stdio_mcp.native_wait_failed",
            "The native stdio MCP process group liveness could not be verified.",
            &error,
        ));
    }
}

#[cfg(windows)]
async fn wait_for_process_group_absence(_process_id: u32) -> UseResult<()> {
    Ok(())
}

fn native_provider_id() -> String {
    format!(
        "a3s-native-stdio-{}-{}",
        std::env::consts::OS,
        std::env::consts::ARCH
    )
}

fn native_io_error(code: &'static str, message: &'static str, error: &io::Error) -> UseError {
    let mut failure =
        native_error(code, message).with_detail("ioKind", format!("{:?}", error.kind()));
    if let Some(os_code) = error.raw_os_error() {
        failure = failure.with_detail("osCode", os_code);
    }
    failure
}

fn native_error(code: &'static str, message: &'static str) -> UseError {
    UseError::new(code, message)
}
