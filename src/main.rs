use std::path::PathBuf;
use std::process;
use std::time::Duration;

use clap::{Parser, Subcommand};

use midi_studio_loader::{api, bootloader, halfkay, selector, serial_reboot, targets, teensy41};

const EXIT_OK: i32 = 0;
const EXIT_NO_DEVICE: i32 = 10;
const EXIT_INVALID_HEX: i32 = 11;
const EXIT_WRITE_FAILED: i32 = 12;
const EXIT_AMBIGUOUS: i32 = 13;
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

    /// Try to enter HalfKay bootloader without the button.
    Reboot(RebootArgs),

    /// List detected HalfKay devices.
    List(ListArgs),
}

#[derive(Parser)]
struct FlashArgs {
    /// Path to Intel HEX firmware.
    hex: PathBuf,

    /// Flash every detected target sequentially.
    #[arg(long, conflicts_with = "device")]
    all: bool,

    /// Select a specific target (e.g. serial:COM6, halfkay:<path>, index:0).
    #[arg(long, conflicts_with = "all")]
    device: Option<String>,

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

    /// Prefer a specific serial port name (e.g. COM6) when selecting among multiple devices.
    #[arg(long)]
    serial_port: Option<String>,

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

#[derive(Parser)]
struct RebootArgs {
    /// Max time to wait for HalfKay to appear (0 = forever).
    #[arg(long, default_value_t = 60000)]
    wait_timeout_ms: u64,

    /// Reboot every detected target sequentially.
    #[arg(long, conflicts_with = "device")]
    all: bool,

    /// Select a specific target (e.g. serial:COM6, halfkay:<path>, index:0).
    #[arg(long, conflicts_with = "all")]
    device: Option<String>,

    /// Prefer a specific serial port name (e.g. COM6).
    #[arg(long)]
    serial_port: Option<String>,

    /// Emit JSON line events to stdout.
    #[arg(long)]
    json: bool,

    /// More logs to stderr.
    #[arg(long, short)]
    verbose: bool,
}

fn main() {
    let cli = Cli::parse();

    let exit_code = match cli.command {
        Command::Flash(args) => cmd_flash(args),
        Command::List(args) => cmd_list(args),
        Command::Reboot(args) => cmd_reboot(args),
    };

    process::exit(exit_code);
}

fn cmd_reboot(args: RebootArgs) -> i32 {
    let wait_timeout = if args.wait_timeout_ms == 0 {
        None
    } else {
        Some(Duration::from_millis(args.wait_timeout_ms))
    };

    if args.json {
        emit_json(&JsonEvent::status("reboot_start"));
    }

    let targets = match targets::discover_targets() {
        Ok(t) => t,
        Err(e) => {
            let msg = format!("target discovery failed: {e}");
            if args.json {
                emit_json(
                    &JsonEvent::status("error")
                        .with_u64("code", EXIT_UNEXPECTED as u64)
                        .with_str("message", &msg),
                );
            }
            eprintln!("error: {msg}");
            return EXIT_UNEXPECTED;
        }
    };

    if targets.is_empty() {
        if args.json {
            emit_json(
                &JsonEvent::status("error")
                    .with_u64("code", EXIT_NO_DEVICE as u64)
                    .with_str("message", "no targets found"),
            );
        }
        eprintln!("No targets found");
        return EXIT_NO_DEVICE;
    }

    let selected: Vec<targets::Target> = if args.all {
        targets.clone()
    } else if let Some(sel) = args.device.as_deref() {
        let parsed = match selector::parse_selector(sel) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("error: {e}");
                return EXIT_AMBIGUOUS;
            }
        };
        let idx = match selector::resolve_one(&parsed, &targets) {
            Ok(i) => i,
            Err(e) => {
                eprintln!("error: {e}");
                return EXIT_AMBIGUOUS;
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
            eprintln!(
                "error: multiple HalfKay devices detected ({}); use --device or --all",
                halfkay_only.len()
            );
            return EXIT_AMBIGUOUS;
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
                eprintln!("error: preferred serial port not found: {port}");
                return EXIT_NO_DEVICE;
            }
        } else if targets.len() == 1 {
            vec![targets[0].clone()]
        } else {
            eprintln!(
                "error: multiple targets detected ({}); use --device or --all",
                targets.len()
            );
            return EXIT_AMBIGUOUS;
        }
    };

    let mut any_failed = false;
    let mut any_ambiguous = false;

    for t in selected {
        let target_id = t.id();

        if args.json {
            emit_json(
                &JsonEvent::status("target_start")
                    .with_str("target_id", &target_id)
                    .with_str(
                        "kind",
                        match t.kind() {
                            targets::TargetKind::HalfKay => "halfkay",
                            targets::TargetKind::Serial => "serial",
                        },
                    ),
            );
        } else if args.verbose {
            eprintln!("target: {target_id}");
        }

        match t {
            targets::Target::HalfKay(hk) => {
                if args.json {
                    emit_json(
                        &JsonEvent::status("halfkay_open")
                            .with_str("target_id", &target_id)
                            .with_str("path", &hk.path),
                    );
                } else {
                    eprintln!("HalfKay open: {}", hk.path);
                }
            }
            targets::Target::Serial(s) => {
                let before = match halfkay::list_paths() {
                    Ok(v) => v.into_iter().collect::<std::collections::HashSet<String>>(),
                    Err(e) => {
                        any_failed = true;
                        eprintln!("error: HalfKay list failed: {e}");
                        continue;
                    }
                };

                if let Err(e) = serial_reboot::soft_reboot_port(&s.port_name) {
                    any_failed = true;
                    eprintln!("error: soft reboot failed on {}: {e}", s.port_name);
                    continue;
                }

                if args.json {
                    emit_json(
                        &JsonEvent::status("soft_reboot")
                            .with_str("target_id", &target_id)
                            .with_str("port", &s.port_name),
                    );
                }

                let timeout = wait_timeout.unwrap_or_else(|| Duration::from_secs(60));
                match bootloader::wait_for_new_halfkay(&before, timeout, Duration::from_millis(50))
                {
                    Ok(path) => {
                        if args.json {
                            emit_json(
                                &JsonEvent::status("halfkay_appeared")
                                    .with_str("target_id", &target_id)
                                    .with_str("path", &path),
                            );
                        } else {
                            eprintln!("HalfKay appeared: {path}");
                        }
                    }
                    Err(bootloader::WaitHalfKayError::Ambiguous { count }) => {
                        any_failed = true;
                        any_ambiguous = true;
                        eprintln!(
                            "error: multiple new HalfKay devices appeared ({count}); use --device"
                        );
                    }
                    Err(e) => {
                        any_failed = true;
                        eprintln!("error: {e}");
                    }
                }
            }
        }

        if args.json {
            emit_json(
                &JsonEvent::status("target_done")
                    .with_str("target_id", &target_id)
                    .with_u64("ok", 1),
            );
        }
    }

    if any_ambiguous {
        return EXIT_AMBIGUOUS;
    }
    if any_failed {
        return EXIT_NO_DEVICE;
    }
    EXIT_OK
}

fn cmd_list(args: ListArgs) -> i32 {
    match targets::discover_targets() {
        Ok(ts) => {
            if args.json {
                for (i, t) in ts.into_iter().enumerate() {
                    let mut v = serde_json::to_value(&t)
                        .unwrap_or_else(|_| serde_json::Value::Object(serde_json::Map::new()));
                    if let serde_json::Value::Object(obj) = &mut v {
                        obj.insert("index".to_string(), serde_json::Value::from(i as u64));
                        obj.insert("id".to_string(), serde_json::Value::from(t.id()));
                    }
                    println!(
                        "{}",
                        serde_json::to_string(&v).unwrap_or_else(|_| "{}".to_string())
                    );
                }
            } else if ts.is_empty() {
                eprintln!(
                    "No targets found (HalfKay {:04X}:{:04X} or PJRC USB serial)",
                    teensy41::VID,
                    teensy41::PID_HALFKAY
                );
            } else {
                for (i, t) in ts.iter().enumerate() {
                    match t {
                        targets::Target::HalfKay(hk) => {
                            eprintln!("[{i}] halfkay {} {:04X}:{:04X}", t.id(), hk.vid, hk.pid);
                        }
                        targets::Target::Serial(s) => {
                            eprintln!(
                                "[{i}] serial  {} {:04X}:{:04X} {}",
                                t.id(),
                                s.vid,
                                s.pid,
                                s.product.as_deref().unwrap_or("")
                            );
                        }
                    }
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

    let opts = api::FlashOptions {
        wait: args.wait,
        wait_timeout,
        no_reboot: args.no_reboot,
        retries: args.retries,
        serial_port: args.serial_port.clone(),
        ..Default::default()
    };

    let selection = if args.all {
        api::FlashSelection::All
    } else if let Some(sel) = args.device.clone() {
        api::FlashSelection::Device(sel)
    } else {
        api::FlashSelection::Auto
    };

    let r = api::flash_teensy41_with_selection(&args.hex, &opts, selection, |ev| {
        handle_flash_event(&args, ev)
    });
    match r {
        Ok(()) => EXIT_OK,
        Err(e) => {
            let code = match e.kind() {
                api::FlashErrorKind::NoDevice => EXIT_NO_DEVICE,
                api::FlashErrorKind::AmbiguousTarget => EXIT_AMBIGUOUS,
                api::FlashErrorKind::InvalidHex => EXIT_INVALID_HEX,
                api::FlashErrorKind::WriteFailed => EXIT_WRITE_FAILED,
                api::FlashErrorKind::Unexpected => EXIT_UNEXPECTED,
            };
            emit_error(&args, code, &e.to_string());
            code
        }
    }
}

fn handle_flash_event(args: &FlashArgs, ev: api::FlashEvent) {
    match ev {
        api::FlashEvent::DiscoverStart => {
            if args.json {
                emit_json(&JsonEvent::status("discover_start"));
            } else if args.verbose {
                eprintln!("discover targets...");
            }
        }
        api::FlashEvent::TargetDetected { index, target } => {
            if args.json {
                emit_json(
                    &JsonEvent::status("target_detected")
                        .with_u64("index", index as u64)
                        .with_str("target_id", &target.id())
                        .with_str(
                            "kind",
                            match target.kind() {
                                targets::TargetKind::HalfKay => "halfkay",
                                targets::TargetKind::Serial => "serial",
                            },
                        ),
                );
            } else if args.verbose {
                eprintln!("target[{index}]: {}", target.id());
            }
        }
        api::FlashEvent::DiscoverDone { count } => {
            if args.json {
                emit_json(&JsonEvent::status("discover_done").with_u64("count", count as u64));
            }
        }
        api::FlashEvent::TargetSelected { target_id } => {
            if args.json {
                emit_json(&JsonEvent::status("target_selected").with_str("target_id", &target_id));
            } else if args.verbose {
                eprintln!("selected: {target_id}");
            }
        }
        api::FlashEvent::HexLoaded { bytes, blocks } => {
            if args.verbose && !args.json {
                eprintln!("Loaded {} bytes ({} blocks) for Teensy 4.1", bytes, blocks);
            }
            if args.json {
                emit_json(
                    &JsonEvent::status("hex_loaded")
                        .with_u64("bytes", bytes as u64)
                        .with_u64("blocks", blocks as u64),
                );
            }
        }
        api::FlashEvent::TargetStart { target_id, kind } => {
            if args.json {
                emit_json(
                    &JsonEvent::status("target_start")
                        .with_str("target_id", &target_id)
                        .with_str(
                            "kind",
                            match kind {
                                targets::TargetKind::HalfKay => "halfkay",
                                targets::TargetKind::Serial => "serial",
                            },
                        ),
                );
            } else if args.verbose {
                eprintln!("target start: {target_id}");
            }
        }
        api::FlashEvent::TargetDone {
            target_id,
            ok,
            message,
        } => {
            if args.json {
                let mut ev = JsonEvent::status("target_done")
                    .with_str("target_id", &target_id)
                    .with_u64("ok", if ok { 1 } else { 0 });
                if let Some(m) = &message {
                    ev = ev.with_str("message", m);
                }
                emit_json(&ev);
            } else if args.verbose {
                if ok {
                    eprintln!("target done: {target_id}");
                } else {
                    eprintln!(
                        "target failed: {target_id}: {}",
                        message.unwrap_or_default()
                    );
                }
            }
        }
        api::FlashEvent::SoftReboot { target_id, port } => {
            if args.verbose && !args.json {
                eprintln!("Soft reboot via serial: {port} (baud=134)");
            }
            if args.json {
                emit_json(
                    &JsonEvent::status("soft_reboot")
                        .with_str("target_id", &target_id)
                        .with_str("port", &port),
                );
            }
        }
        api::FlashEvent::SoftRebootSkipped { target_id, error } => {
            if args.verbose {
                eprintln!("soft reboot skipped: {error}");
            }
            if args.json {
                emit_json(
                    &JsonEvent::status("soft_reboot_skipped")
                        .with_str("target_id", &target_id)
                        .with_str("message", &error),
                );
            }
        }
        api::FlashEvent::HalfKayAppeared { target_id, path } => {
            if args.json {
                emit_json(
                    &JsonEvent::status("halfkay_appeared")
                        .with_str("target_id", &target_id)
                        .with_str("path", &path),
                );
            } else if args.verbose {
                eprintln!("HalfKay appeared: {path}");
            }
        }
        api::FlashEvent::HalfKayOpen { target_id, path } => {
            if args.json {
                emit_json(
                    &JsonEvent::status("halfkay_open")
                        .with_str("target_id", &target_id)
                        .with_str("path", &path),
                );
            } else if args.verbose {
                eprintln!("HalfKay open: {path}");
            }
        }
        api::FlashEvent::Block {
            target_id,
            index,
            total,
            addr,
        } => {
            if args.json {
                emit_json(
                    &JsonEvent::status("block")
                        .with_str("target_id", &target_id)
                        .with_u64("i", index as u64)
                        .with_u64("n", total as u64)
                        .with_u64("addr", addr as u64),
                );
            } else if args.verbose {
                eprintln!("program block {}/{} @ 0x{:06X}", index + 1, total, addr);
            }
        }
        api::FlashEvent::Retry {
            target_id,
            addr,
            attempt,
            retries,
            error,
        } => {
            if args.verbose {
                eprintln!(
                    "write failed at 0x{addr:06X} (attempt {attempt}/{retries}) - reopening: {error}"
                );
            }
            if args.json {
                emit_json(
                    &JsonEvent::status("retry")
                        .with_str("target_id", &target_id)
                        .with_u64("addr", addr as u64)
                        .with_u64("attempt", attempt as u64)
                        .with_u64("retries", retries as u64)
                        .with_str("error", &error),
                );
            }
        }
        api::FlashEvent::Boot { target_id } => {
            if args.json {
                emit_json(&JsonEvent::status("boot").with_str("target_id", &target_id));
            }
        }
        api::FlashEvent::Done { target_id } => {
            if args.json {
                emit_json(&JsonEvent::status("done").with_str("target_id", &target_id));
            }
        }
    }
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

#[derive(serde::Serialize)]
struct JsonEvent {
    schema: u32,
    event: &'static str,
    #[serde(flatten)]
    fields: std::collections::BTreeMap<&'static str, serde_json::Value>,
}

impl JsonEvent {
    fn status(event: &'static str) -> Self {
        Self {
            schema: 1,
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
