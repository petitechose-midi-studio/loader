use std::time::Duration;

use midi_studio_loader::{bridge_control, targets};

use crate::cli;
use crate::exit_codes;
use crate::output::json::JsonEvent;
use crate::output::{target_to_value, Output};

pub fn run(args: cli::DoctorArgs, out: &mut dyn Output) -> i32 {
    let service_id = args
        .bridge_service_id
        .clone()
        .unwrap_or_else(bridge_control::default_service_id_for_platform);

    let targets = match targets::discover_targets() {
        Ok(t) => t,
        Err(e) => {
            out.error(
                exit_codes::EXIT_UNEXPECTED,
                &format!("target discovery failed: {e}"),
            );
            return exit_codes::EXIT_UNEXPECTED;
        }
    };

    let svc_status = bridge_control::service_status(&service_id);
    let procs = bridge_control::list_oc_bridge_processes();

    let control_timeout = Duration::from_millis(args.bridge_control_timeout_ms);
    let control = if args.no_bridge_control {
        None
    } else {
        Some(bridge_control::control_status(
            args.bridge_control_port,
            control_timeout,
        ))
    };

    if args.json {
        let targets_val = serde_json::Value::Array(
            targets
                .iter()
                .enumerate()
                .map(|(i, t)| target_to_value(i, t))
                .collect(),
        );

        let mut ev = JsonEvent::status("doctor")
            .with_str("service_id", &service_id)
            .with_value("targets", targets_val)
            .with_value(
                "processes",
                serde_json::to_value(&procs)
                    .unwrap_or_else(|_| serde_json::Value::Array(Vec::new())),
            );

        ev = match &control {
            None => ev.with_u64("control_checked", 0),
            Some(Ok(st)) => ev.with_u64("control_checked", 1).with_value(
                "control",
                serde_json::to_value(st)
                    .unwrap_or_else(|_| serde_json::Value::Object(serde_json::Map::new())),
            ),
            Some(Err(e)) => ev
                .with_u64("control_checked", 1)
                .with_str("control_error", &e.to_string()),
        };

        ev = match svc_status {
            Ok(s) => ev.with_value(
                "service_status",
                serde_json::to_value(s).unwrap_or_else(|_| serde_json::Value::from("unknown")),
            ),
            Err(e) => ev.with_str("service_error", &e.to_string()),
        };

        out.json_event(ev);
        return exit_codes::EXIT_OK;
    }

    out.human_line("midi-studio-loader doctor");
    out.human_line(&format!("targets: {}", targets.len()));
    for (i, t) in targets.iter().enumerate() {
        match t {
            targets::Target::HalfKay(hk) => {
                out.human_line(&format!(
                    "[{i}] halfkay {} {:04X}:{:04X}",
                    t.id(),
                    hk.vid,
                    hk.pid
                ));
            }
            targets::Target::Serial(s) => {
                out.human_line(&format!(
                    "[{i}] serial  {} {:04X}:{:04X} {}",
                    t.id(),
                    s.vid,
                    s.pid,
                    s.product.as_deref().unwrap_or("")
                ));
            }
        }
    }

    out.human_line(&format!(
        "oc-bridge control: 127.0.0.1:{} (timeout {}ms){}",
        args.bridge_control_port,
        args.bridge_control_timeout_ms,
        if args.no_bridge_control {
            " (skipped)"
        } else {
            ""
        }
    ));
    if let Some(control) = control {
        match control {
            Ok(st) => {
                out.human_line(&format!(
                    "  ok={} paused={} serial_open={:?}",
                    st.ok, st.paused, st.serial_open
                ));
                if let Some(m) = st.message {
                    out.human_line(&format!("  message: {m}"));
                }
            }
            Err(e) => out.human_line(&format!("  error: {e}")),
        }
    }

    out.human_line(&format!("oc-bridge service: {service_id}"));
    match svc_status {
        Ok(s) => out.human_line(&format!("  status: {s:?}")),
        Err(e) => out.human_line(&format!("  error: {e}")),
    }

    out.human_line(&format!("oc-bridge processes: {}", procs.len()));
    for p in procs {
        out.human_line(&format!(
            "  pid={} restartable={} exe={}",
            p.pid,
            if p.restartable { "yes" } else { "no" },
            p.exe.as_deref().unwrap_or("")
        ));
    }

    exit_codes::EXIT_OK
}
