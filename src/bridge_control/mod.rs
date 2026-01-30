use std::time::Duration;

use serde::Serialize;
use thiserror::Error;

mod cmd;
mod ipc;
mod process;
mod service;

pub use ipc::{control_status, BridgeControlStatus};
pub use process::{list_oc_bridge_processes, OcBridgeProcessInfo};
pub use service::{default_service_id_for_platform, service_status, ServiceStatus};

#[derive(Debug, Clone)]
pub struct BridgeControlOptions {
    /// Enable automatic bridge pause/resume.
    pub enabled: bool,

    /// Override the OS service identifier.
    ///
    /// - Windows: service name (e.g. "OpenControlBridge")
    /// - Linux: systemd user unit (e.g. "open-control-bridge")
    /// - macOS: launchd label (e.g. "com.petitechose.open-control-bridge")
    pub service_id: Option<String>,

    /// Max time to wait for stop/start.
    pub timeout: Duration,

    /// Local control port for oc-bridge IPC (pause/resume).
    ///
    /// When available, we prefer this over stopping the OS service.
    pub control_port: u16,

    /// Max time to wait for oc-bridge IPC.
    pub control_timeout: Duration,
}

impl Default for BridgeControlOptions {
    fn default() -> Self {
        Self {
            enabled: true,
            service_id: None,
            timeout: Duration::from_secs(5),
            control_port: 7999,
            // oc-bridge pause waits for the serial port to actually close (ack), so
            // this needs to cover that round-trip.
            control_timeout: Duration::from_millis(2500),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BridgePauseMethod {
    Control,
    Service,
    Process,
}

#[derive(Debug, Clone, Serialize)]
pub struct BridgePauseInfo {
    pub method: BridgePauseMethod,
    pub id: String,
    pub pids: Vec<u32>,
}

#[derive(Debug, Clone)]
pub enum BridgePauseOutcome {
    Paused(BridgePauseInfo),
    Skipped(BridgePauseSkipReason),
    Failed(BridgeControlErrorInfo),
}

#[derive(Debug, Clone)]
pub enum BridgePauseSkipReason {
    Disabled,
    NotRunning,
    NotInstalled,
    ProcessNotRestartable,
}

#[derive(Debug, Clone)]
pub struct BridgeControlErrorInfo {
    pub message: String,
    pub hint: Option<String>,
}

#[derive(Error, Debug)]
pub enum BridgeControlError {
    #[error("command failed: {cmd}: {message}")]
    CommandFailed { cmd: String, message: String },

    #[error("timeout")]
    Timeout,

    #[error("process restart unavailable")]
    ProcessRestartUnavailable,
}

#[derive(Debug, Clone)]
enum ResumePlan {
    Control { port: u16, timeout: Duration },
    Service { id: String },
    Processes { cmds: Vec<process::RelaunchCmd> },
}

#[derive(Debug)]
pub struct BridgeGuard {
    resume: Option<ResumePlan>,
    timeout: Duration,
}

impl BridgeGuard {
    pub fn resume_hint(&self) -> Option<String> {
        match self.resume.as_ref() {
            Some(ResumePlan::Control { port, .. }) => {
                Some(format!("Try: oc-bridge ctl resume --control-port {port}"))
            }
            Some(ResumePlan::Service { id }) => Some(service::hint_start_service(id)),
            _ => None,
        }
    }

    pub fn resume(&mut self) -> Result<(), BridgeControlError> {
        let Some(plan) = self.resume.clone() else {
            return Ok(());
        };
        match resume(plan.clone(), self.timeout) {
            Ok(()) => {
                self.resume = None;
                Ok(())
            }
            Err(e) => {
                // Keep the plan for Drop() best-effort retries.
                self.resume = Some(plan);
                Err(e)
            }
        }
    }
}

impl Drop for BridgeGuard {
    fn drop(&mut self) {
        let _ = self.resume();
    }
}

pub struct BridgePause {
    pub guard: Option<BridgeGuard>,
    pub outcome: BridgePauseOutcome,
}

pub fn pause_oc_bridge(opts: &BridgeControlOptions) -> BridgePause {
    if !opts.enabled {
        return BridgePause {
            guard: None,
            outcome: BridgePauseOutcome::Skipped(BridgePauseSkipReason::Disabled),
        };
    }

    let service_id = opts
        .service_id
        .clone()
        .unwrap_or_else(default_service_id_for_platform);

    // 0) Prefer IPC pause/resume when available.
    if let Ok(()) = ipc::control_pause(opts.control_port, opts.control_timeout) {
        let info = BridgePauseInfo {
            method: BridgePauseMethod::Control,
            id: format!("127.0.0.1:{}", opts.control_port),
            pids: Vec::new(),
        };
        return BridgePause {
            guard: Some(BridgeGuard {
                resume: Some(ResumePlan::Control {
                    port: opts.control_port,
                    timeout: opts.control_timeout,
                }),
                timeout: opts.timeout,
            }),
            outcome: BridgePauseOutcome::Paused(info),
        };
    }

    // 1) service-first
    match service::service_status(&service_id) {
        Ok(ServiceStatus::Running) => match service::stop_service(&service_id, opts.timeout) {
            Ok(()) => {
                let info = BridgePauseInfo {
                    method: BridgePauseMethod::Service,
                    id: service_id.clone(),
                    pids: Vec::new(),
                };
                return BridgePause {
                    guard: Some(BridgeGuard {
                        resume: Some(ResumePlan::Service { id: service_id }),
                        timeout: opts.timeout,
                    }),
                    outcome: BridgePauseOutcome::Paused(info),
                };
            }
            Err(e) => {
                return BridgePause {
                    guard: None,
                    outcome: BridgePauseOutcome::Failed(BridgeControlErrorInfo {
                        message: format!("unable to stop bridge service '{service_id}': {e}"),
                        hint: Some(service::hint_stop_service(&service_id)),
                    }),
                };
            }
        },
        Ok(ServiceStatus::Stopped) => {
            return BridgePause {
                guard: None,
                outcome: BridgePauseOutcome::Skipped(BridgePauseSkipReason::NotRunning),
            }
        }
        Ok(ServiceStatus::NotInstalled) => {}
        Err(e) => {
            // Fail-safe: don't guess and kill processes if we can't even query the service.
            return BridgePause {
                guard: None,
                outcome: BridgePauseOutcome::Failed(BridgeControlErrorInfo {
                    message: format!("unable to query bridge service '{service_id}': {e}"),
                    hint: Some(service::hint_query_service(&service_id)),
                }),
            };
        }
    }

    // 2) process fallback (only if restartable)
    match process::pause_process_fallback(opts.timeout) {
        process::ProcessPauseOutcome::Paused {
            info,
            relaunch_cmds,
        } => BridgePause {
            guard: Some(BridgeGuard {
                resume: Some(ResumePlan::Processes {
                    cmds: relaunch_cmds,
                }),
                timeout: opts.timeout,
            }),
            outcome: BridgePauseOutcome::Paused(info),
        },
        process::ProcessPauseOutcome::Skipped(reason) => BridgePause {
            guard: None,
            outcome: BridgePauseOutcome::Skipped(reason),
        },
        process::ProcessPauseOutcome::Failed(error) => BridgePause {
            guard: None,
            outcome: BridgePauseOutcome::Failed(error),
        },
    }
}

fn resume(plan: ResumePlan, timeout: Duration) -> Result<(), BridgeControlError> {
    match plan {
        ResumePlan::Control { port, timeout } => ipc::control_resume(port, timeout),
        ResumePlan::Service { id } => service::start_service(&id, timeout),
        ResumePlan::Processes { cmds } => process::resume_processes(&cmds),
    }
}
