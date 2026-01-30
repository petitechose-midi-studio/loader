use std::time::Duration;

use serde::Serialize;

#[cfg(feature = "cli")]
use serde::Deserialize;

use super::BridgeControlError;

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

pub(super) fn control_pause(port: u16, timeout: Duration) -> Result<(), BridgeControlError> {
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

pub(super) fn control_resume(port: u16, timeout: Duration) -> Result<(), BridgeControlError> {
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

#[cfg(feature = "cli")]
#[derive(Debug, Deserialize)]
struct ControlRespJson {
    ok: bool,
    paused: bool,
    #[serde(default)]
    serial_open: Option<bool>,
    #[serde(default)]
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

    #[cfg(feature = "cli")]
    {
        if let Ok(v) = serde_json::from_str::<ControlRespJson>(line) {
            return Ok(ControlResp {
                ok: v.ok,
                paused: v.paused,
                serial_open: v.serial_open,
                message: v.message,
            });
        }
    }

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
