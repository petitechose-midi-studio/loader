use midi_studio_loader::{targets, teensy41};

use crate::cli;
use crate::exit_codes;
use crate::output::{target_to_value, Output};

pub fn run(args: cli::ListArgs, out: &mut dyn Output) -> i32 {
    match targets::discover_targets() {
        Ok(ts) => {
            if args.json {
                for (i, t) in ts.iter().enumerate() {
                    out.json_line(target_to_value(i, t));
                }
            } else if ts.is_empty() {
                out.human_line(&format!(
                    "No targets found (HalfKay {:04X}:{:04X} or PJRC USB serial)",
                    teensy41::VID,
                    teensy41::PID_HALFKAY
                ));
            } else {
                for (i, t) in ts.iter().enumerate() {
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
            }
            exit_codes::EXIT_OK
        }
        Err(e) => {
            out.error(exit_codes::EXIT_UNEXPECTED, &e.to_string());
            exit_codes::EXIT_UNEXPECTED
        }
    }
}
