use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::Serialize;
use sysinfo::{Process, ProcessRefreshKind, RefreshKind, System, UpdateKind};
use thiserror::Error;

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
    Processes { cmds: Vec<RelaunchCmd> },
}

#[derive(Debug, Clone)]
struct RelaunchCmd {
    exe: PathBuf,
    args: Vec<String>,
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
            Some(ResumePlan::Service { id }) => Some(hint_start_service(id)),
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
    if let Ok(()) = control_pause(opts.control_port, opts.control_timeout) {
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
    match service_status(&service_id) {
        Ok(ServiceStatus::Running) => match stop_service(&service_id, opts.timeout) {
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
                    outcome: BridgePauseOutcome::Failed(error_info(
                        format!("unable to stop bridge service '{service_id}': {e}"),
                        Some(hint_stop_service(&service_id)),
                    )),
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
                outcome: BridgePauseOutcome::Failed(error_info(
                    format!("unable to query bridge service '{service_id}': {e}"),
                    Some(hint_query_service(&service_id)),
                )),
            };
        }
    }

    // 2) process fallback (only if restartable)
    let mut system = System::new_with_specifics(
        RefreshKind::new().with_processes(
            ProcessRefreshKind::new()
                .with_exe(UpdateKind::OnlyIfNotSet)
                .with_cmd(UpdateKind::OnlyIfNotSet),
        ),
    );

    let processes = find_oc_bridge_processes(&system);
    if processes.is_empty() {
        return BridgePause {
            guard: None,
            outcome: BridgePauseOutcome::Skipped(BridgePauseSkipReason::NotRunning),
        };
    }

    let mut relaunch_cmds: Vec<RelaunchCmd> = Vec::new();
    let mut pids: Vec<u32> = Vec::new();

    for p in &processes {
        let pid_u32 = p.pid_u32;
        pids.push(pid_u32);

        let Some(exe) = p.exe.clone() else {
            return BridgePause {
                guard: None,
                outcome: BridgePauseOutcome::Skipped(BridgePauseSkipReason::ProcessNotRestartable),
            };
        };

        let args = p
            .cmd
            .clone()
            .unwrap_or_else(|| vec!["--daemon".to_string(), "--no-relaunch".to_string()]);

        relaunch_cmds.push(RelaunchCmd { exe, args });
    }

    // Terminate all oc-bridge processes.
    if let Err(e) = stop_processes(&mut system, &processes, opts.timeout) {
        return BridgePause {
            guard: None,
            outcome: BridgePauseOutcome::Failed(error_info(
                format!("unable to stop oc-bridge process: {e}"),
                None,
            )),
        };
    }

    let info = BridgePauseInfo {
        method: BridgePauseMethod::Process,
        id: "oc-bridge".to_string(),
        pids,
    };

    BridgePause {
        guard: Some(BridgeGuard {
            resume: Some(ResumePlan::Processes {
                cmds: relaunch_cmds,
            }),
            timeout: opts.timeout,
        }),
        outcome: BridgePauseOutcome::Paused(info),
    }
}

fn error_info(message: String, hint: Option<String>) -> BridgeControlErrorInfo {
    BridgeControlErrorInfo { message, hint }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceStatus {
    Running,
    Stopped,
    NotInstalled,
}

pub fn default_service_id_for_platform() -> String {
    // Mirrors midi-studio/core/script/pio/oc_service.py.
    #[cfg(windows)]
    {
        "OpenControlBridge".to_string()
    }
    #[cfg(target_os = "linux")]
    {
        "open-control-bridge".to_string()
    }
    #[cfg(target_os = "macos")]
    {
        "com.petitechose.open-control-bridge".to_string()
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        "oc-bridge".to_string()
    }
}

fn hint_stop_service(service_id: &str) -> String {
    #[cfg(windows)]
    {
        format!("Try: sc stop {service_id}")
    }
    #[cfg(target_os = "linux")]
    {
        format!("Try: systemctl --user stop {service_id}")
    }
    #[cfg(target_os = "macos")]
    {
        format!("Try: launchctl stop {service_id}")
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        "".to_string()
    }
}

fn hint_query_service(service_id: &str) -> String {
    #[cfg(windows)]
    {
        format!("Try: sc query {service_id}")
    }
    #[cfg(target_os = "linux")]
    {
        format!("Try: systemctl --user status {service_id}")
    }
    #[cfg(target_os = "macos")]
    {
        format!("Try: launchctl list {service_id}")
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        "".to_string()
    }
}

fn hint_start_service(service_id: &str) -> String {
    #[cfg(windows)]
    {
        format!("Try: sc start {service_id}")
    }
    #[cfg(target_os = "linux")]
    {
        format!("Try: systemctl --user start {service_id}")
    }
    #[cfg(target_os = "macos")]
    {
        format!("Try: launchctl start {service_id}")
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        "".to_string()
    }
}

pub fn service_status(service_id: &str) -> Result<ServiceStatus, BridgeControlError> {
    #[cfg(windows)]
    {
        let out = run_capture("sc", &["query", service_id], None)?;
        if out.status_code != 0 {
            // 1060 = service not installed.
            if out.text.contains("1060") {
                return Ok(ServiceStatus::NotInstalled);
            }
            return Err(BridgeControlError::CommandFailed {
                cmd: format!("sc query {service_id}"),
                message: out.text,
            });
        }

        match parse_sc_state(&out.text) {
            Some(1) => Ok(ServiceStatus::Stopped),
            Some(4) => Ok(ServiceStatus::Running),
            Some(_) => Ok(ServiceStatus::Running),
            None => Err(BridgeControlError::CommandFailed {
                cmd: format!("sc query {service_id}"),
                message: "unable to parse service state".to_string(),
            }),
        }
    }

    #[cfg(target_os = "linux")]
    {
        let out = run_capture(
            "systemctl",
            &["--user", "is-active", service_id],
            Some(linux_user_env_fix()),
        )?;

        let first_line = out
            .text
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();

        match first_line.as_str() {
            "active" | "activating" | "deactivating" => return Ok(ServiceStatus::Running),
            "inactive" | "failed" => return Ok(ServiceStatus::Stopped),
            "unknown" => return Ok(ServiceStatus::NotInstalled),
            _ => {}
        }

        // "inactive" and "unknown" are both non-zero; treat missing unit as not installed.
        if out.text.contains("not-found") || out.text.contains("could not be found") {
            return Ok(ServiceStatus::NotInstalled);
        }

        if out.status_code == 0 {
            return Ok(ServiceStatus::Running);
        }
        Ok(ServiceStatus::Stopped)
    }

    #[cfg(target_os = "macos")]
    {
        let out = run_capture("launchctl", &["list", service_id], None)?;

        if out.status_code == 0 {
            // `launchctl list <label>` prints a single row when the label exists.
            // Common format: "PID Status Label" where PID is a number when running
            // or "-" when loaded but not running.
            if let Some(s) = parse_launchctl_list_status(&out.text) {
                return Ok(s);
            }
            // Fallback: be conservative and treat as Running.
            return Ok(ServiceStatus::Running);
        }

        // launchctl doesn't provide a stable exit code distinction between
        // "not installed" and "installed but stopped". We treat the common
        // "could not find" case as NotInstalled, otherwise Stopped.
        let lower = out.text.to_ascii_lowercase();
        if lower.contains("could not find") || lower.contains("no such process") {
            return Ok(ServiceStatus::NotInstalled);
        }
        Ok(ServiceStatus::Stopped)
    }

    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        let _ = service_id;
        Ok(ServiceStatus::NotInstalled)
    }
}

#[cfg(target_os = "macos")]
fn parse_launchctl_list_status(text: &str) -> Option<ServiceStatus> {
    let line = text.lines().find(|l| !l.trim().is_empty())?;
    let first = line.split_whitespace().next()?;

    if first == "-" {
        return Some(ServiceStatus::Stopped);
    }

    if let Ok(pid) = first.parse::<u32>() {
        if pid > 0 {
            return Some(ServiceStatus::Running);
        }
        // Unexpected (PID 0). Avoid false "Stopped" and keep conservative.
        return Some(ServiceStatus::Running);
    }

    None
}

fn stop_service(service_id: &str, timeout: Duration) -> Result<(), BridgeControlError> {
    // If the service doesn't exist, stopping it is equivalent to success.
    if service_status(service_id)? == ServiceStatus::NotInstalled {
        return Ok(());
    }

    let cmd = stop_service_cmd(service_id);

    #[cfg(windows)]
    let wait_res = wait_for_windows_service_state(service_id, 1, timeout);
    #[cfg(not(windows))]
    let wait_res = wait_for_service_stopped(service_id, timeout);

    match wait_res {
        Ok(()) => Ok(()),
        Err(wait_err) => Err(service_action_error(
            "stop", service_id, timeout, cmd, wait_err,
        )),
    }
}

fn start_service(service_id: &str, timeout: Duration) -> Result<(), BridgeControlError> {
    // Starting a service that isn't installed is a hard error.
    if service_status(service_id)? == ServiceStatus::NotInstalled {
        return Err(BridgeControlError::CommandFailed {
            cmd: start_service_cmd_string(service_id),
            message: "service is not installed".to_string(),
        });
    }

    let cmd = start_service_cmd(service_id);

    #[cfg(windows)]
    let wait_res = wait_for_windows_service_state(service_id, 4, timeout);
    #[cfg(not(windows))]
    let wait_res = wait_for_service_running(service_id, timeout);

    match wait_res {
        Ok(()) => Ok(()),
        Err(wait_err) => Err(service_action_error(
            "start", service_id, timeout, cmd, wait_err,
        )),
    }
}

#[cfg(windows)]
fn wait_for_windows_service_state(
    service_id: &str,
    desired: u32,
    timeout: Duration,
) -> Result<(), BridgeControlError> {
    let start = Instant::now();
    loop {
        let out = run_capture("sc", &["query", service_id], None)?;
        if out.status_code != 0 {
            // 1060 = service not installed.
            if out.text.contains("1060") {
                return Err(BridgeControlError::CommandFailed {
                    cmd: format!("sc query {service_id}"),
                    message: "service not installed".to_string(),
                });
            }
            return Err(BridgeControlError::CommandFailed {
                cmd: format!("sc query {service_id}"),
                message: out.text,
            });
        }

        let state = parse_sc_state(&out.text).ok_or_else(|| BridgeControlError::CommandFailed {
            cmd: format!("sc query {service_id}"),
            message: "unable to parse service state".to_string(),
        })?;

        if state == desired {
            return Ok(());
        }

        if start.elapsed() >= timeout {
            return Err(BridgeControlError::Timeout);
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

#[cfg(not(windows))]
fn wait_for_service_stopped(service_id: &str, timeout: Duration) -> Result<(), BridgeControlError> {
    wait_for_service_state(service_id, timeout, |s| {
        matches!(s, ServiceStatus::Stopped | ServiceStatus::NotInstalled)
    })
}

#[cfg(not(windows))]
fn wait_for_service_running(service_id: &str, timeout: Duration) -> Result<(), BridgeControlError> {
    wait_for_service_state(service_id, timeout, |s| matches!(s, ServiceStatus::Running))
}

#[cfg(not(windows))]
fn wait_for_service_state<F>(
    service_id: &str,
    timeout: Duration,
    mut predicate: F,
) -> Result<(), BridgeControlError>
where
    F: FnMut(ServiceStatus) -> bool,
{
    let start = Instant::now();
    loop {
        let status = service_status(service_id)?;
        if predicate(status) {
            return Ok(());
        }
        if start.elapsed() >= timeout {
            return Err(BridgeControlError::Timeout);
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn service_action_error(
    action: &str,
    service_id: &str,
    timeout: Duration,
    cmd_result: Result<CmdOutput, BridgeControlError>,
    wait_err: BridgeControlError,
) -> BridgeControlError {
    let cmd = match action {
        "stop" => stop_service_cmd_string(service_id),
        "start" => start_service_cmd_string(service_id),
        _ => format!("{action} {service_id}"),
    };

    let mut message = if matches!(&wait_err, BridgeControlError::Timeout) {
        format!("timeout waiting for service to {action} (timeout {timeout:?})")
    } else {
        format!("error while waiting for service to {action}: {wait_err} (timeout {timeout:?})")
    };

    if let Ok(status) = service_status(service_id) {
        message.push_str(&format!("\nservice status: {status:?}"));
    }

    match cmd_result {
        Ok(out) => {
            message.push_str(&format!("\ncommand exit code: {}", out.status_code));
            if !out.text.trim().is_empty() {
                message.push_str("\ncommand output:\n");
                message.push_str(out.text.trim_end());
            }
        }
        Err(e) => {
            message.push_str("\ncommand error: ");
            message.push_str(&e.to_string());
        }
    }

    BridgeControlError::CommandFailed { cmd, message }
}

fn stop_service_cmd_string(service_id: &str) -> String {
    #[cfg(windows)]
    {
        format!("sc stop {service_id}")
    }
    #[cfg(target_os = "linux")]
    {
        format!("systemctl --user stop {service_id}")
    }
    #[cfg(target_os = "macos")]
    {
        format!("launchctl stop {service_id}")
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        format!("stop {service_id}")
    }
}

fn start_service_cmd_string(service_id: &str) -> String {
    #[cfg(windows)]
    {
        format!("sc start {service_id}")
    }
    #[cfg(target_os = "linux")]
    {
        format!("systemctl --user start {service_id}")
    }
    #[cfg(target_os = "macos")]
    {
        format!("launchctl start {service_id}")
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        format!("start {service_id}")
    }
}

fn stop_service_cmd(service_id: &str) -> Result<CmdOutput, BridgeControlError> {
    #[cfg(windows)]
    {
        run_capture("sc", &["stop", service_id], None)
    }
    #[cfg(target_os = "linux")]
    {
        run_capture(
            "systemctl",
            &["--user", "stop", service_id],
            Some(linux_user_env_fix()),
        )
    }
    #[cfg(target_os = "macos")]
    {
        run_capture("launchctl", &["stop", service_id], None)
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        let _ = service_id;
        Ok(CmdOutput {
            status_code: 0,
            text: String::new(),
        })
    }
}

fn start_service_cmd(service_id: &str) -> Result<CmdOutput, BridgeControlError> {
    #[cfg(windows)]
    {
        run_capture("sc", &["start", service_id], None)
    }
    #[cfg(target_os = "linux")]
    {
        run_capture(
            "systemctl",
            &["--user", "start", service_id],
            Some(linux_user_env_fix()),
        )
    }
    #[cfg(target_os = "macos")]
    {
        run_capture("launchctl", &["start", service_id], None)
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        let _ = service_id;
        Ok(CmdOutput {
            status_code: 0,
            text: String::new(),
        })
    }
}

#[cfg(windows)]
fn parse_sc_state(text: &str) -> Option<u32> {
    // We avoid localized keywords (RUNNING/STOPPED). Look for a line containing STATE/ETAT
    // and parse the first integer after ':'.
    for line in text.lines() {
        let upper = line.to_ascii_uppercase();
        if !upper.contains("STATE") && !upper.contains("ETAT") {
            continue;
        }

        let (_, rhs) = line.split_once(':')?;
        let rhs = rhs.trim_start();
        let mut num = String::new();
        for ch in rhs.chars() {
            if ch.is_ascii_digit() {
                num.push(ch);
            } else if !num.is_empty() {
                break;
            }
        }
        if num.is_empty() {
            continue;
        }
        if let Ok(v) = num.parse::<u32>() {
            return Some(v);
        }
    }
    None
}

fn resume(plan: ResumePlan, timeout: Duration) -> Result<(), BridgeControlError> {
    match plan {
        ResumePlan::Control { port, timeout } => control_resume(port, timeout),
        ResumePlan::Service { id } => start_service(&id, timeout),
        ResumePlan::Processes { cmds } => {
            for c in cmds {
                let mut cmd = Command::new(&c.exe);
                cmd.args(&c.args)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null());
                let _ = cmd.spawn().map_err(|e| BridgeControlError::CommandFailed {
                    cmd: format!("spawn {:?}", c.exe),
                    message: e.to_string(),
                })?;
            }
            Ok(())
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct BridgeControlStatus {
    pub ok: bool,
    pub paused: bool,
    pub serial_open: Option<bool>,
    pub message: Option<String>,
}

pub fn control_status(
    port: u16,
    timeout: Duration,
) -> Result<BridgeControlStatus, BridgeControlError> {
    let resp = control_send(port, "status", timeout)?;
    Ok(BridgeControlStatus {
        ok: resp.ok,
        paused: resp.paused,
        serial_open: resp.serial_open,
        message: resp.message,
    })
}

fn control_pause(port: u16, timeout: Duration) -> Result<(), BridgeControlError> {
    let resp = control_send(port, "pause", timeout)?;
    if !resp.ok {
        return Err(BridgeControlError::CommandFailed {
            cmd: format!("oc-bridge control pause (port {port})"),
            message: resp.message.unwrap_or_else(|| "unknown error".to_string()),
        });
    }
    if !resp.paused {
        return Err(BridgeControlError::CommandFailed {
            cmd: format!("oc-bridge control pause (port {port})"),
            message: "bridge did not enter paused state".to_string(),
        });
    }
    if let Some(open) = resp.serial_open {
        if open {
            return Err(BridgeControlError::CommandFailed {
                cmd: format!("oc-bridge control pause (port {port})"),
                message: "bridge reports serial_open=true after pause".to_string(),
            });
        }
    }
    Ok(())
}

fn control_resume(port: u16, timeout: Duration) -> Result<(), BridgeControlError> {
    let resp = control_send(port, "resume", timeout)?;
    if !resp.ok {
        return Err(BridgeControlError::CommandFailed {
            cmd: format!("oc-bridge control resume (port {port})"),
            message: resp.message.unwrap_or_else(|| "unknown error".to_string()),
        });
    }
    if resp.paused {
        return Err(BridgeControlError::CommandFailed {
            cmd: format!("oc-bridge control resume (port {port})"),
            message: "bridge still paused after resume".to_string(),
        });
    }
    Ok(())
}

#[derive(Debug)]
struct ControlResp {
    ok: bool,
    paused: bool,
    serial_open: Option<bool>,
    message: Option<String>,
}

fn control_send(
    port: u16,
    cmd: &str,
    timeout: Duration,
) -> Result<ControlResp, BridgeControlError> {
    use std::io::{Read, Write};
    use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream};

    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
    let mut stream = TcpStream::connect_timeout(&addr, timeout).map_err(|e| {
        BridgeControlError::CommandFailed {
            cmd: format!("oc-bridge control connect (port {port})"),
            message: e.to_string(),
        }
    })?;
    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));

    let req = format!("{{\"cmd\":\"{cmd}\"}}\n");
    stream
        .write_all(req.as_bytes())
        .map_err(|e| BridgeControlError::CommandFailed {
            cmd: format!("oc-bridge control write (port {port})"),
            message: e.to_string(),
        })?;
    stream.flush().ok();

    let mut out = String::new();
    stream
        .read_to_string(&mut out)
        .map_err(|e| BridgeControlError::CommandFailed {
            cmd: format!("oc-bridge control read (port {port})"),
            message: e.to_string(),
        })?;

    parse_control_response(&out)
}

fn parse_control_response(s: &str) -> Result<ControlResp, BridgeControlError> {
    let line = s.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    let compact: String = line.chars().filter(|c| !c.is_whitespace()).collect();

    let ok = compact.contains("\"ok\":true");
    let paused = compact.contains("\"paused\":true");
    let serial_open = if compact.contains("\"serial_open\":true") {
        Some(true)
    } else if compact.contains("\"serial_open\":false") {
        Some(false)
    } else {
        None
    };

    // Best-effort extraction of a message (optional).
    let message = extract_json_string_field(&compact, "message");

    Ok(ControlResp {
        ok,
        paused,
        serial_open,
        message,
    })
}

fn extract_json_string_field(s: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\":\"");
    let idx = s.find(&needle)?;
    let rest = &s[(idx + needle.len())..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

#[derive(Debug)]
struct CmdOutput {
    status_code: i32,
    text: String,
}

fn run_capture(
    program: &str,
    args: &[&str],
    env: Option<Vec<(String, String)>>,
) -> Result<CmdOutput, BridgeControlError> {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(env) = env {
        for (k, v) in env {
            cmd.env(k, v);
        }
    }
    let out = cmd
        .output()
        .map_err(|e| BridgeControlError::CommandFailed {
            cmd: format!("{program} {}", args.join(" ")),
            message: e.to_string(),
        })?;

    let mut text = String::new();
    text.push_str(&String::from_utf8_lossy(&out.stdout));
    text.push_str(&String::from_utf8_lossy(&out.stderr));

    Ok(CmdOutput {
        status_code: out.status.code().unwrap_or(-1),
        text,
    })
}

#[cfg(target_os = "linux")]
fn linux_user_env_fix() -> Vec<(String, String)> {
    // Mirrors midi-studio/core/script/pio/oc_service.py.
    let mut out: Vec<(String, String)> = Vec::new();

    if std::env::var_os("XDG_RUNTIME_DIR").is_none() {
        if let Ok(uid) = std::env::var("UID") {
            out.push(("XDG_RUNTIME_DIR".to_string(), format!("/run/user/{uid}")));
        }
    }

    if std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_none() {
        if let Ok(uid) = std::env::var("UID") {
            out.push((
                "DBUS_SESSION_BUS_ADDRESS".to_string(),
                format!("unix:path=/run/user/{uid}/bus"),
            ));
        }
    }

    out
}

#[derive(Debug, Clone)]
struct OcBridgeProcess {
    pid_u32: u32,
    exe: Option<PathBuf>,
    cmd: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct OcBridgeProcessInfo {
    pub pid: u32,
    pub exe: Option<String>,
    pub cmd: Option<Vec<String>>,
    pub restartable: bool,
}

pub fn list_oc_bridge_processes() -> Vec<OcBridgeProcessInfo> {
    let system = System::new_with_specifics(
        RefreshKind::new().with_processes(
            ProcessRefreshKind::new()
                .with_exe(UpdateKind::OnlyIfNotSet)
                .with_cmd(UpdateKind::OnlyIfNotSet),
        ),
    );

    find_oc_bridge_processes(&system)
        .into_iter()
        .map(|p| OcBridgeProcessInfo {
            pid: p.pid_u32,
            exe: p.exe.as_ref().map(|e| e.to_string_lossy().to_string()),
            cmd: p.cmd.clone(),
            restartable: p.exe.is_some(),
        })
        .collect()
}

fn find_oc_bridge_processes(system: &System) -> Vec<OcBridgeProcess> {
    system
        .processes()
        .iter()
        .filter_map(|(pid, p)| {
            let name = p.name();
            if !is_oc_bridge_name(name) {
                return None;
            }

            let exe = match p.exe() {
                Some(e) if !e.as_os_str().is_empty() => Some(e.to_path_buf()),
                _ => None,
            };

            let cmd = {
                let c = p.cmd();
                if c.is_empty() {
                    None
                } else {
                    // sysinfo typically includes argv[0] as the executable. We store argv[1..]
                    // so we can restart from the known exe path.
                    Some(c.iter().skip(1).cloned().collect())
                }
            };

            Some(OcBridgeProcess {
                pid_u32: pid.as_u32(),
                exe,
                cmd,
            })
        })
        .collect()
}

fn is_oc_bridge_name(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n == "oc-bridge" || n == "oc-bridge.exe"
}

fn stop_processes(
    system: &mut System,
    procs: &[OcBridgeProcess],
    timeout: Duration,
) -> Result<(), BridgeControlError> {
    // Best-effort: ask processes to exit.
    for p in procs {
        if let Some(proc_) = get_process_by_pid(system, p.pid_u32) {
            let _ = proc_.kill();
        }
    }

    let start = Instant::now();
    loop {
        system.refresh_processes_specifics(ProcessRefreshKind::new());
        let still_running = procs
            .iter()
            .any(|p| get_process_by_pid(system, p.pid_u32).is_some());
        if !still_running {
            return Ok(());
        }

        if start.elapsed() >= timeout {
            return Err(BridgeControlError::Timeout);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn get_process_by_pid(system: &System, pid_u32: u32) -> Option<&Process> {
    system.processes().iter().find_map(|(pid, p)| {
        if pid.as_u32() == pid_u32 {
            Some(p)
        } else {
            None
        }
    })
}

#[cfg(test)]
mod tests {
    #[cfg(windows)]
    use super::parse_sc_state;

    #[cfg(windows)]
    #[test]
    fn test_parse_sc_state_running() {
        let s = "STATE              : 4  RUNNING\r\n";
        assert_eq!(parse_sc_state(s), Some(4));
    }

    #[cfg(windows)]
    #[test]
    fn test_parse_sc_state_stopped() {
        let s = "STATE              : 1  STOPPED\r\n";
        assert_eq!(parse_sc_state(s), Some(1));
    }
}
