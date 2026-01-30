use std::process::{Command, Stdio};

use super::BridgeControlError;

#[derive(Debug)]
pub(super) struct CmdOutput {
    pub(super) status_code: i32,
    pub(super) text: String,
}

pub(super) fn run_capture(
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
pub(super) fn linux_user_env_fix() -> Vec<(String, String)> {
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
