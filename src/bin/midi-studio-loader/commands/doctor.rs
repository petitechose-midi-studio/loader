use std::time::Duration;

use midi_studio_loader::{bridge_control, targets};

use crate::cli;
use crate::exit_codes;
use crate::output::{DoctorReport, Event, Reporter};

pub fn run(args: cli::DoctorArgs, out: &mut dyn Reporter) -> i32 {
    let service_id = args
        .bridge_service_id
        .clone()
        .unwrap_or_else(bridge_control::default_service_id_for_platform);

    let targets = match targets::discover_targets() {
        Ok(t) => t,
        Err(e) => {
            out.emit(Event::Error {
                code: exit_codes::EXIT_UNEXPECTED,
                message: format!("target discovery failed: {e}"),
            });
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

    let (control_checked, control_status, control_error) = match control {
        None => (false, None, None),
        Some(Ok(st)) => (true, Some(st), None),
        Some(Err(e)) => (true, None, Some(e.to_string())),
    };

    let (service_status, service_error) = match svc_status {
        Ok(s) => (Some(s), None),
        Err(e) => (None, Some(e.to_string())),
    };

    let report = DoctorReport {
        service_id,
        targets,
        control_port: args.bridge_control_port,
        control_timeout_ms: args.bridge_control_timeout_ms,
        control_checked,
        control: control_status,
        control_error,
        service_status,
        service_error,
        processes: procs,
    };

    out.emit(Event::Doctor(report));

    exit_codes::EXIT_OK
}
