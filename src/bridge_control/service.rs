use std::time::{Duration, Instant};

use serde::Serialize;

use super::cmd;
use super::BridgeControlError;

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

pub(super) fn hint_stop_service(service_id: &str) -> String {
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

pub(super) fn hint_query_service(service_id: &str) -> String {
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

pub(super) fn hint_start_service(service_id: &str) -> String {
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
        let out = cmd::run_capture("sc", &["query", service_id], None)?;
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
        let out = cmd::run_capture(
            "systemctl",
            &["--user", "is-active", service_id],
            Some(cmd::linux_user_env_fix()),
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
        let out = cmd::run_capture("launchctl", &["list", service_id], None)?;

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

pub(super) fn stop_service(service_id: &str, timeout: Duration) -> Result<(), BridgeControlError> {
    // If the service doesn't exist, stopping it is equivalent to success.
    if service_status(service_id)? == ServiceStatus::NotInstalled {
        return Ok(());
    }

    let cmd_out = stop_service_cmd(service_id);

    #[cfg(windows)]
    let wait_res = wait_for_windows_service_state(service_id, 1, timeout);
    #[cfg(not(windows))]
    let wait_res = wait_for_service_stopped(service_id, timeout);

    match wait_res {
        Ok(()) => Ok(()),
        Err(wait_err) => Err(service_action_error(
            "stop", service_id, timeout, cmd_out, wait_err,
        )),
    }
}

pub(super) fn start_service(service_id: &str, timeout: Duration) -> Result<(), BridgeControlError> {
    // Starting a service that isn't installed is a hard error.
    if service_status(service_id)? == ServiceStatus::NotInstalled {
        return Err(BridgeControlError::CommandFailed {
            cmd: start_service_cmd_string(service_id),
            message: "service is not installed".to_string(),
        });
    }

    let cmd_out = start_service_cmd(service_id);

    #[cfg(windows)]
    let wait_res = wait_for_windows_service_state(service_id, 4, timeout);
    #[cfg(not(windows))]
    let wait_res = wait_for_service_running(service_id, timeout);

    match wait_res {
        Ok(()) => Ok(()),
        Err(wait_err) => Err(service_action_error(
            "start", service_id, timeout, cmd_out, wait_err,
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
        let out = cmd::run_capture("sc", &["query", service_id], None)?;
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
    cmd_result: Result<cmd::CmdOutput, BridgeControlError>,
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

fn stop_service_cmd(service_id: &str) -> Result<cmd::CmdOutput, BridgeControlError> {
    #[cfg(windows)]
    {
        cmd::run_capture("sc", &["stop", service_id], None)
    }
    #[cfg(target_os = "linux")]
    {
        cmd::run_capture(
            "systemctl",
            &["--user", "stop", service_id],
            Some(cmd::linux_user_env_fix()),
        )
    }
    #[cfg(target_os = "macos")]
    {
        cmd::run_capture("launchctl", &["stop", service_id], None)
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        let _ = service_id;
        Ok(cmd::CmdOutput {
            status_code: 0,
            text: String::new(),
        })
    }
}

fn start_service_cmd(service_id: &str) -> Result<cmd::CmdOutput, BridgeControlError> {
    #[cfg(windows)]
    {
        cmd::run_capture("sc", &["start", service_id], None)
    }
    #[cfg(target_os = "linux")]
    {
        cmd::run_capture(
            "systemctl",
            &["--user", "start", service_id],
            Some(cmd::linux_user_env_fix()),
        )
    }
    #[cfg(target_os = "macos")]
    {
        cmd::run_capture("launchctl", &["start", service_id], None)
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        let _ = service_id;
        Ok(cmd::CmdOutput {
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
