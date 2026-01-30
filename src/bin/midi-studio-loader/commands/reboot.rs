use std::collections::HashSet;
use std::time::Duration;

use midi_studio_loader::{bootloader, halfkay, selector, serial_reboot, targets};

use crate::cli;
use crate::exit_codes;
use crate::output::json::JsonEvent;
use crate::output::Output;

pub fn run(args: cli::RebootArgs, out: &mut dyn Output) -> i32 {
    let wait_timeout = if args.wait_timeout_ms == 0 {
        None
    } else {
        Some(Duration::from_millis(args.wait_timeout_ms))
    };

    if args.json {
        out.json_event(JsonEvent::status("reboot_start"));
    }

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

    if targets.is_empty() {
        out.error(exit_codes::EXIT_NO_DEVICE, "no targets found");
        return exit_codes::EXIT_NO_DEVICE;
    }

    let selected: Vec<targets::Target> = if args.all {
        targets.clone()
    } else if let Some(sel) = args.device.as_deref() {
        let parsed = match selector::parse_selector(sel) {
            Ok(s) => s,
            Err(e) => {
                out.error(exit_codes::EXIT_AMBIGUOUS, &e.to_string());
                return exit_codes::EXIT_AMBIGUOUS;
            }
        };
        let idx = match selector::resolve_one(&parsed, &targets) {
            Ok(i) => i,
            Err(e) => {
                out.error(exit_codes::EXIT_AMBIGUOUS, &e.to_string());
                return exit_codes::EXIT_AMBIGUOUS;
            }
        };
        vec![targets[idx].clone()]
    } else {
        let halfkay_only: Vec<targets::Target> = targets
            .iter()
            .filter(|t| t.kind() == targets::TargetKind::HalfKay)
            .cloned()
            .collect();

        if halfkay_only.len() == 1 {
            vec![halfkay_only[0].clone()]
        } else if !halfkay_only.is_empty() {
            out.error(
                exit_codes::EXIT_AMBIGUOUS,
                &format!(
                    "multiple HalfKay devices detected ({}); use --device or --all",
                    halfkay_only.len()
                ),
            );
            return exit_codes::EXIT_AMBIGUOUS;
        } else if let Some(port) = args.serial_port.as_deref() {
            let matches: Vec<targets::Target> = targets
                .iter()
                .filter_map(|t| match t {
                    targets::Target::Serial(s) if s.port_name == port => Some(t.clone()),
                    _ => None,
                })
                .collect();
            if matches.len() == 1 {
                vec![matches[0].clone()]
            } else {
                out.error(
                    exit_codes::EXIT_NO_DEVICE,
                    &format!("preferred serial port not found: {port}"),
                );
                return exit_codes::EXIT_NO_DEVICE;
            }
        } else if targets.len() == 1 {
            vec![targets[0].clone()]
        } else {
            out.error(
                exit_codes::EXIT_AMBIGUOUS,
                &format!(
                    "multiple targets detected ({}); use --device or --all",
                    targets.len()
                ),
            );
            return exit_codes::EXIT_AMBIGUOUS;
        }
    };

    let needs_serial = selected
        .iter()
        .any(|t| matches!(t, targets::Target::Serial(_)));

    let mut bridge_guard: Option<midi_studio_loader::bridge_control::BridgeGuard> = None;
    if needs_serial {
        let bridge = midi_studio_loader::bridge_control::BridgeControlOptions {
            enabled: !args.bridge.no_bridge_control,
            service_id: args.bridge.bridge_service_id.clone(),
            timeout: Duration::from_millis(args.bridge.bridge_timeout_ms),
            control_port: args.bridge.bridge_control_port,
            control_timeout: Duration::from_millis(args.bridge.bridge_control_timeout_ms),
        };

        if args.json {
            out.json_event(JsonEvent::status("bridge_pause_start"));
        }

        let paused = midi_studio_loader::bridge_control::pause_oc_bridge(&bridge);
        match &paused.outcome {
            midi_studio_loader::bridge_control::BridgePauseOutcome::Paused(info) => {
                if args.json {
                    let method = match info.method {
                        midi_studio_loader::bridge_control::BridgePauseMethod::Control => "control",
                        midi_studio_loader::bridge_control::BridgePauseMethod::Service => "service",
                        midi_studio_loader::bridge_control::BridgePauseMethod::Process => "process",
                    };
                    out.json_event(
                        JsonEvent::status("bridge_paused")
                            .with_str("method", method)
                            .with_str("id", &info.id)
                            .with_value(
                                "pids",
                                serde_json::Value::Array(
                                    info.pids
                                        .iter()
                                        .map(|p| serde_json::Value::from(*p as u64))
                                        .collect(),
                                ),
                            ),
                    );
                }
            }
            midi_studio_loader::bridge_control::BridgePauseOutcome::Skipped(_) => {}
            midi_studio_loader::bridge_control::BridgePauseOutcome::Failed(e) => {
                if args.json {
                    let mut ev =
                        JsonEvent::status("bridge_pause_failed").with_str("message", &e.message);
                    if let Some(hint) = &e.hint {
                        ev = ev.with_str("hint", hint);
                    }
                    out.json_event(ev);
                }
            }
        }
        bridge_guard = paused.guard;
    }

    let mut any_failed = false;
    let mut any_ambiguous = false;

    for t in selected {
        let target_id = t.id();

        if args.json {
            out.json_event(
                JsonEvent::status("target_start")
                    .with_str("target_id", &target_id)
                    .with_str(
                        "kind",
                        match t.kind() {
                            targets::TargetKind::HalfKay => "halfkay",
                            targets::TargetKind::Serial => "serial",
                        },
                    ),
            );
        }

        match t {
            targets::Target::HalfKay(hk) => {
                if args.json {
                    out.json_event(
                        JsonEvent::status("halfkay_open")
                            .with_str("target_id", &target_id)
                            .with_str("path", &hk.path),
                    );
                } else {
                    out.human_line(&format!("HalfKay open: {}", hk.path));
                }
            }
            targets::Target::Serial(s) => {
                let before = match halfkay::list_paths() {
                    Ok(v) => v.into_iter().collect::<HashSet<String>>(),
                    Err(e) => {
                        any_failed = true;
                        out.error(
                            exit_codes::EXIT_UNEXPECTED,
                            &format!("HalfKay list failed: {e}"),
                        );
                        continue;
                    }
                };

                if let Err(e) = serial_reboot::soft_reboot_port(&s.port_name) {
                    any_failed = true;
                    out.error(
                        exit_codes::EXIT_UNEXPECTED,
                        &format!("soft reboot failed on {}: {e}", s.port_name),
                    );
                    continue;
                }

                if args.json {
                    out.json_event(
                        JsonEvent::status("soft_reboot")
                            .with_str("target_id", &target_id)
                            .with_str("port", &s.port_name),
                    );
                }

                let timeout = wait_timeout.unwrap_or_else(|| Duration::from_secs(60));
                match bootloader::wait_for_new_halfkay(&before, timeout, Duration::from_millis(50))
                {
                    Ok(path) => {
                        if args.json {
                            out.json_event(
                                JsonEvent::status("halfkay_appeared")
                                    .with_str("target_id", &target_id)
                                    .with_str("path", &path),
                            );
                        } else {
                            out.human_line(&format!("HalfKay appeared: {path}"));
                        }
                    }
                    Err(bootloader::WaitHalfKayError::Ambiguous { count }) => {
                        any_failed = true;
                        any_ambiguous = true;
                        out.error(
                            exit_codes::EXIT_AMBIGUOUS,
                            &format!(
                                "multiple new HalfKay devices appeared ({count}); use --device"
                            ),
                        );
                    }
                    Err(e) => {
                        any_failed = true;
                        out.error(exit_codes::EXIT_UNEXPECTED, &e.to_string());
                    }
                }
            }
        }

        if args.json {
            out.json_event(
                JsonEvent::status("target_done")
                    .with_str("target_id", &target_id)
                    .with_u64("ok", 1),
            );
        }
    }

    let exit_code = if any_ambiguous {
        exit_codes::EXIT_AMBIGUOUS
    } else if any_failed {
        exit_codes::EXIT_NO_DEVICE
    } else {
        exit_codes::EXIT_OK
    };

    if let Some(mut g) = bridge_guard {
        if args.json {
            out.json_event(JsonEvent::status("bridge_resume_start"));
        }

        let hint = g.resume_hint();
        match g.resume() {
            Ok(()) => {
                if args.json {
                    out.json_event(JsonEvent::status("bridge_resumed"));
                }
            }
            Err(e) => {
                if args.json {
                    let mut ev = JsonEvent::status("bridge_resume_failed")
                        .with_str("message", &e.to_string());
                    if let Some(hint) = hint {
                        ev = ev.with_str("hint", &hint);
                    }
                    out.json_event(ev);
                }
            }
        }
    }

    exit_code
}
