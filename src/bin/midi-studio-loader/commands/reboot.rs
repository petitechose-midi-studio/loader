use midi_studio_loader::{api, reboot_api};

use crate::cli;
use crate::context;
use crate::exit_codes;
use crate::output::Output;

pub fn run(args: cli::RebootArgs, out: &mut dyn Output) -> i32 {
    let selection = if args.all {
        api::FlashSelection::All
    } else if let Some(sel) = args.device.clone() {
        api::FlashSelection::Device(sel)
    } else {
        api::FlashSelection::Auto
    };

    let opts = reboot_api::RebootOptions {
        serial_port: args.serial_port.clone(),
        wait_timeout: context::wait_timeout(args.wait_timeout_ms),
        bridge: context::bridge_opts(&args.bridge),
        ..Default::default()
    };

    let r = reboot_api::reboot_teensy41_with_selection(&opts, selection, |ev| out.flash_event(ev));
    match r {
        Ok(()) => exit_codes::EXIT_OK,
        Err(e) => {
            let code = match e.kind() {
                reboot_api::RebootErrorKind::NoDevice => exit_codes::EXIT_NO_DEVICE,
                reboot_api::RebootErrorKind::AmbiguousTarget => exit_codes::EXIT_AMBIGUOUS,
                reboot_api::RebootErrorKind::Unexpected => exit_codes::EXIT_UNEXPECTED,
            };
            out.error(code, &e.to_string());
            if code == exit_codes::EXIT_AMBIGUOUS {
                out.ambiguous_help();
            }
            code
        }
    }
}
