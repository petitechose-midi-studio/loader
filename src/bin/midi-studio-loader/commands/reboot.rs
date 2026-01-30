use midi_studio_loader::selector;
use midi_studio_loader::{api, reboot_api};

use crate::cli;
use crate::context;
use crate::exit_codes;
use crate::output::{Event, Reporter};

pub fn run(args: cli::RebootArgs, out: &mut dyn Reporter) -> i32 {
    let selection = if args.all {
        api::FlashSelection::All
    } else if let Some(sel) = args.device.clone() {
        match selector::parse_selector(&sel) {
            Ok(s) => api::FlashSelection::Device(s),
            Err(e) => {
                out.emit(Event::Error {
                    code: exit_codes::EXIT_AMBIGUOUS,
                    message: e.to_string(),
                });
                out.emit(Event::HintAmbiguousTargets);
                return exit_codes::EXIT_AMBIGUOUS;
            }
        }
    } else {
        api::FlashSelection::Auto
    };

    let opts = reboot_api::RebootOptions {
        serial_port: args.serial_port.clone(),
        wait_timeout: context::wait_timeout(args.wait_timeout_ms),
        bridge: context::bridge_opts(&args.bridge),
        ..Default::default()
    };

    let r = reboot_api::reboot_teensy41_with_selection(&opts, selection, |ev| {
        out.emit(Event::Operation(ev))
    });
    match r {
        Ok(()) => exit_codes::EXIT_OK,
        Err(e) => {
            let code = match e.kind() {
                reboot_api::RebootErrorKind::NoDevice => exit_codes::EXIT_NO_DEVICE,
                reboot_api::RebootErrorKind::AmbiguousTarget => exit_codes::EXIT_AMBIGUOUS,
                reboot_api::RebootErrorKind::Unexpected => exit_codes::EXIT_UNEXPECTED,
            };
            out.emit(Event::Error {
                code,
                message: e.to_string(),
            });
            if matches!(
                e,
                reboot_api::RebootError::NoTargets
                    | reboot_api::RebootError::TargetNotFound { .. }
                    | reboot_api::RebootError::AmbiguousTarget { .. }
            ) {
                out.emit(Event::HintAmbiguousTargets);
            }
            code
        }
    }
}
