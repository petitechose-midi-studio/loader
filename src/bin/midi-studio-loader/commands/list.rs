use midi_studio_loader::targets;

use crate::cli;
use crate::exit_codes;
use crate::output::{Event, Reporter};

pub fn run(_args: cli::ListArgs, out: &mut dyn Reporter) -> i32 {
    match targets::discover_targets() {
        Ok(ts) => {
            out.emit(Event::ListTargets(ts));
            exit_codes::EXIT_OK
        }
        Err(e) => {
            out.emit(Event::Error {
                code: exit_codes::EXIT_UNEXPECTED,
                message: e.to_string(),
            });
            exit_codes::EXIT_UNEXPECTED
        }
    }
}
