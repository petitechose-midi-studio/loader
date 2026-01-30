use std::path::Path;

use midi_studio_loader::api;

use crate::cli;
use crate::context;
use crate::exit_codes;
use crate::output::{DryRunSummary, Event, Reporter};

pub fn run(args: cli::FlashArgs, out: &mut dyn Reporter) -> i32 {
    let wait_timeout = context::wait_timeout(args.wait_timeout_ms);

    let bridge = context::bridge_opts(&args.bridge);

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

    let r = api::flash_teensy41_with_selection(&args.hex, &opts, selection, |ev| {
        out.emit(Event::Flash(ev))
    });

    match r {
        Ok(()) => exit_codes::EXIT_OK,
        Err(e) => {
            let code = map_flash_error(&e);
            out.emit(Event::Error {
                code,
                message: e.to_string(),
            });
            if code == exit_codes::EXIT_AMBIGUOUS {
                out.emit(Event::HintAmbiguousTargets);
            }
            code
        }
    }
}

fn dry_run(
    hex: &Path,
    opts: &api::FlashOptions,
    selection: api::FlashSelection,
    out: &mut dyn Reporter,
) -> i32 {
    let r =
        api::plan_teensy41_with_selection(hex, opts, selection, |ev| out.emit(Event::Flash(ev)));
    match r {
        Ok(plan) => {
            let summary = DryRunSummary {
                bytes: plan.firmware.byte_count,
                blocks: plan.firmware.num_blocks,
                blocks_to_write: plan.firmware.blocks_to_write.len(),
                target_ids: plan.selected_targets.iter().map(|t| t.id()).collect(),
                needs_serial: plan.needs_serial,
                bridge_enabled: opts.bridge.enabled,
                bridge_control_port: opts.bridge.control_port,
            };
            out.emit(Event::DryRun(summary));
            exit_codes::EXIT_OK
        }
        Err(e) => {
            let code = map_flash_error(&e);
            out.emit(Event::Error {
                code,
                message: e.to_string(),
            });
            if code == exit_codes::EXIT_AMBIGUOUS {
                out.emit(Event::HintAmbiguousTargets);
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
