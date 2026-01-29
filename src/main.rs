use std::path::PathBuf;
use std::process;
use std::time::Duration;

use clap::{Parser, Subcommand};

mod halfkay;
mod hex;
mod teensy41;

const EXIT_OK: i32 = 0;
const EXIT_NO_DEVICE: i32 = 10;
const EXIT_INVALID_HEX: i32 = 11;
const EXIT_WRITE_FAILED: i32 = 12;
const EXIT_UNEXPECTED: i32 = 20;

#[derive(Parser)]
#[command(name = "midi-studio-loader")]
#[command(about = "Teensy 4.1 flasher CLI (HalfKay)")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Flash an Intel HEX to a Teensy 4.1 in HalfKay bootloader mode.
    Flash(FlashArgs),

    /// List detected HalfKay devices.
    List(ListArgs),
}

#[derive(Parser)]
struct FlashArgs {
    /// Path to Intel HEX firmware.
    hex: PathBuf,

    /// Wait for HalfKay bootloader to appear.
    #[arg(long)]
    wait: bool,

    /// Max time to wait for device (0 = forever).
    #[arg(long, default_value_t = 0)]
    wait_timeout_ms: u64,

    /// Do not reboot after programming.
    #[arg(long)]
    no_reboot: bool,

    /// Retries per block on write failure.
    #[arg(long, default_value_t = 3)]
    retries: u32,

    /// Emit JSON line events to stdout.
    #[arg(long)]
    json: bool,

    /// More logs to stderr.
    #[arg(long, short)]
    verbose: bool,
}

#[derive(Parser)]
struct ListArgs {
    /// Emit JSON line output.
    #[arg(long)]
    json: bool,
}

fn main() {
    let cli = Cli::parse();

    let exit_code = match cli.command {
        Command::Flash(args) => cmd_flash(args),
        Command::List(args) => cmd_list(args),
    };

    process::exit(exit_code);
}

fn cmd_list(args: ListArgs) -> i32 {
    match halfkay::list_devices() {
        Ok(devices) => {
            if args.json {
                for d in devices {
                    println!(
                        "{}",
                        serde_json::to_string(&d).unwrap_or_else(|_| "{}".to_string())
                    );
                }
            } else if devices.is_empty() {
                eprintln!(
                    "No HalfKay devices found (VID:PID {:04X}:{:04X})",
                    teensy41::VID,
                    teensy41::PID_HALFKAY
                );
            } else {
                for d in devices {
                    eprintln!("HalfKay {:04X}:{:04X} {}", d.vid, d.pid, d.path);
                }
            }
            EXIT_OK
        }
        Err(e) => {
            eprintln!("error: {e}");
            EXIT_UNEXPECTED
        }
    }
}

fn cmd_flash(args: FlashArgs) -> i32 {
    let wait_timeout = if args.wait_timeout_ms == 0 {
        None
    } else {
        Some(Duration::from_millis(args.wait_timeout_ms))
    };

    match run_flash(&args, wait_timeout) {
        Ok(()) => EXIT_OK,
        Err(FlashError::NoDevice(msg)) => {
            emit_error(&args, EXIT_NO_DEVICE, &msg);
            EXIT_NO_DEVICE
        }
        Err(FlashError::InvalidHex(msg)) => {
            emit_error(&args, EXIT_INVALID_HEX, &msg);
            EXIT_INVALID_HEX
        }
        Err(FlashError::WriteFailed(msg)) => {
            emit_error(&args, EXIT_WRITE_FAILED, &msg);
            EXIT_WRITE_FAILED
        }
    }
}

fn run_flash(args: &FlashArgs, wait_timeout: Option<Duration>) -> Result<(), FlashError> {
    let fw = hex::FirmwareImage::load_teensy41(&args.hex)
        .map_err(|e| FlashError::InvalidHex(e.to_string()))?;

    if args.verbose && !args.json {
        eprintln!(
            "Loaded {} bytes ({} blocks) for Teensy 4.1",
            fw.byte_count, fw.num_blocks
        );
    }

    if args.json {
        let ev = JsonEvent::status("hex_loaded")
            .with_u64("bytes", fw.byte_count as u64)
            .with_u64("blocks", fw.num_blocks as u64);
        emit_json(&ev);
    }

    let mut dev = halfkay::open_halfkay_device(args.wait, wait_timeout).map_err(|e| {
        FlashError::NoDevice(format!(
            "unable to open HalfKay device (VID:PID {:04X}:{:04X}): {e}",
            teensy41::VID,
            teensy41::PID_HALFKAY
        ))
    })?;

    if args.json {
        emit_json(&JsonEvent::status("halfkay_open"));
    } else if args.verbose {
        eprintln!("HalfKay open: {}", dev.path);
    }

    let total_to_write = fw.blocks_to_write.len();
    for (i, block_addr) in fw.blocks_to_write.iter().copied().enumerate() {
        if args.json {
            emit_json(
                &JsonEvent::status("block")
                    .with_u64("i", i as u64)
                    .with_u64("n", total_to_write as u64)
                    .with_u64("addr", block_addr as u64),
            );
        } else if args.verbose {
            eprintln!(
                "program block {}/{} @ 0x{:06X}",
                i + 1,
                total_to_write,
                block_addr
            );
        }

        let mut attempt = 0;
        loop {
            attempt += 1;
            match halfkay::write_block_teensy41(&dev, &fw, block_addr) {
                Ok(()) => break,
                Err(e) => {
                    if attempt > args.retries {
                        return Err(FlashError::WriteFailed(format!(
                            "write failed at addr=0x{block_addr:06X} after {attempt} attempts: {e}"
                        )));
                    }

                    if args.verbose {
                        eprintln!(
                            "write failed at 0x{block_addr:06X} (attempt {attempt}/{}) - reopening: {e}",
                            args.retries
                        );
                    }

                    std::thread::sleep(Duration::from_millis(150));
                    dev = halfkay::open_halfkay_device(true, Some(Duration::from_secs(10)))
                        .map_err(|e2| {
                            FlashError::WriteFailed(format!(
                                "unable to reopen HalfKay device: {e2}"
                            ))
                        })?;
                    std::thread::sleep(Duration::from_millis(150));
                }
            }
        }
    }

    if !args.no_reboot {
        if args.json {
            emit_json(&JsonEvent::status("boot"));
        }
        // Best-effort (device may disappear quickly).
        let _ = halfkay::boot_teensy41(&dev);
    }

    if args.json {
        emit_json(&JsonEvent::status("done"));
    }

    Ok(())
}

fn emit_error(args: &FlashArgs, code: i32, msg: &str) {
    if args.json {
        let ev = JsonEvent::status("error")
            .with_u64("code", code as u64)
            .with_str("message", msg);
        emit_json(&ev);
    }

    if !args.json || args.verbose {
        eprintln!("error: {msg}");
    }
}

#[derive(Debug)]
enum FlashError {
    NoDevice(String),
    InvalidHex(String),
    WriteFailed(String),
}

#[derive(serde::Serialize)]
struct JsonEvent {
    event: &'static str,
    #[serde(flatten)]
    fields: std::collections::BTreeMap<&'static str, serde_json::Value>,
}

impl JsonEvent {
    fn status(event: &'static str) -> Self {
        Self {
            event,
            fields: std::collections::BTreeMap::new(),
        }
    }

    fn with_u64(mut self, k: &'static str, v: u64) -> Self {
        self.fields.insert(k, serde_json::Value::from(v));
        self
    }

    fn with_str(mut self, k: &'static str, v: &str) -> Self {
        self.fields.insert(k, serde_json::Value::from(v));
        self
    }
}

fn emit_json(ev: &JsonEvent) {
    // JSON lines to stdout.
    println!(
        "{}",
        serde_json::to_string(ev).unwrap_or_else(|_| "{}".to_string())
    );
}
