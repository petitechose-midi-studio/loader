use std::path::Path;
use std::time::Duration;

use midi_studio_loader::api;

use crate::cli;
use crate::exit_codes;
use crate::output::json::JsonEvent;
use crate::output::Output;

pub fn run(args: cli::FlashArgs, out: &mut dyn Output) -> i32 {
    let wait_timeout = if args.wait_timeout_ms == 0 {
        None
    } else {
        Some(Duration::from_millis(args.wait_timeout_ms))
    };

    let bridge = midi_studio_loader::bridge_control::BridgeControlOptions {
        enabled: !args.bridge.no_bridge_control,
        service_id: args.bridge.bridge_service_id.clone(),
        timeout: Duration::from_millis(args.bridge.bridge_timeout_ms),
        control_port: args.bridge.bridge_control_port,
        control_timeout: Duration::from_millis(args.bridge.bridge_control_timeout_ms),
    };

    let opts = api::FlashOptions {
        wait: args.wait,
        wait_timeout,
        no_reboot: args.no_reboot,
        retries: args.retries,
        serial_port: args.serial_port.clone(),
        bridge,
        ..Default::default()
    };

    let selection = if args.all {
        api::FlashSelection::All
    } else if let Some(sel) = args.device.clone() {
        api::FlashSelection::Device(sel)
    } else {
        api::FlashSelection::Auto
    };

    if args.dry_run {
        return dry_run(&args.hex, &opts, selection, out);
    }

    let r =
        api::flash_teensy41_with_selection(&args.hex, &opts, selection, |ev| out.flash_event(ev));

    match r {
        Ok(()) => exit_codes::EXIT_OK,
        Err(e) => {
            let code = map_flash_error(&e);
            out.error(code, &e.to_string());
            if code == exit_codes::EXIT_AMBIGUOUS {
                out.ambiguous_help();
            }
            code
        }
    }
}

fn dry_run(
    hex: &Path,
    opts: &api::FlashOptions,
    selection: api::FlashSelection,
    out: &mut dyn Output,
) -> i32 {
    let r = api::plan_teensy41_with_selection(hex, opts, selection, |ev| out.flash_event(ev));
    match r {
        Ok(plan) => {
            if out.options().json {
                out.json_event(
                    JsonEvent::status("dry_run")
                        .with_u64("bytes", plan.firmware.byte_count as u64)
                        .with_u64("blocks", plan.firmware.num_blocks as u64)
                        .with_u64(
                            "blocks_to_write",
                            plan.firmware.blocks_to_write.len() as u64,
                        )
                        .with_u64("targets", plan.selected_targets.len() as u64)
                        .with_u64("needs_serial", if plan.needs_serial { 1 } else { 0 })
                        .with_value(
                            "target_ids",
                            serde_json::Value::Array(
                                plan.selected_targets
                                    .iter()
                                    .map(|t| serde_json::Value::from(t.id()))
                                    .collect(),
                            ),
                        ),
                );
            } else if !out.options().quiet {
                out.human_line("Dry run OK");
                out.human_line(&format!(
                    "Firmware: {} bytes, blocks_to_write={}/{},",
                    plan.firmware.byte_count,
                    plan.firmware.blocks_to_write.len(),
                    plan.firmware.num_blocks
                ));
                out.human_line(&format!("Targets: {}", plan.selected_targets.len()));
                for t in &plan.selected_targets {
                    out.human_line(&format!("- {}", t.id()));
                }
                if plan.needs_serial && opts.bridge.enabled {
                    out.human_line(&format!(
                        "Bridge: would pause/resume oc-bridge (control port {})",
                        opts.bridge.control_port
                    ));
                }
            }
            exit_codes::EXIT_OK
        }
        Err(e) => {
            let code = map_flash_error(&e);
            out.error(code, &e.to_string());
            if code == exit_codes::EXIT_AMBIGUOUS {
                out.ambiguous_help();
            }
            code
        }
    }
}

fn map_flash_error(e: &api::FlashError) -> i32 {
    match e.kind() {
        api::FlashErrorKind::NoDevice => exit_codes::EXIT_NO_DEVICE,
        api::FlashErrorKind::AmbiguousTarget => exit_codes::EXIT_AMBIGUOUS,
        api::FlashErrorKind::InvalidHex => exit_codes::EXIT_INVALID_HEX,
        api::FlashErrorKind::WriteFailed => exit_codes::EXIT_WRITE_FAILED,
        api::FlashErrorKind::Unexpected => exit_codes::EXIT_UNEXPECTED,
    }
}
