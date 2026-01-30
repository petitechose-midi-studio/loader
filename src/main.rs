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

    /// List detected targets (HalfKay + PJRC USB serial).
    List(ListArgs),

    /// Diagnose target detection and bridge coordination.
    Doctor(DoctorArgs),
}

#[derive(Parser)]
struct BridgeControlArgs {
    /// Disable automatic oc-bridge pause/resume.
    #[arg(long)]
    no_bridge_control: bool,

    /// Max time to wait when stopping/starting the bridge.
    #[arg(long, default_value_t = 5000)]
    bridge_timeout_ms: u64,

    /// Override the bridge service identifier.
    ///
    /// - Windows: service name (default: OpenControlBridge)
    /// - Linux: systemd user unit (default: open-control-bridge)
    /// - macOS: launchd label (default: com.petitechose.open-control-bridge)
    #[arg(long)]
    bridge_service_id: Option<String>,

    /// Local oc-bridge control port (pause/resume IPC).
    #[arg(long, default_value_t = 7999)]
    bridge_control_port: u16,

    /// Max time to wait for oc-bridge IPC.
    #[arg(long, default_value_t = 2500)]
    bridge_control_timeout_ms: u64,
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

    /// Wait for a target to appear (HalfKay or PJRC USB serial).
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

    #[command(flatten)]
    bridge: BridgeControlArgs,

    /// Emit JSON line events to stdout.
    #[arg(long)]
    json: bool,

    /// Validate inputs and selection without flashing.
    #[arg(long)]
    dry_run: bool,

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
struct DoctorArgs {
    /// Skip probing oc-bridge IPC.
    #[arg(long)]
    no_bridge_control: bool,

    /// Override the bridge service identifier.
    ///
    /// - Windows: service name (default: OpenControlBridge)
    /// - Linux: systemd user unit (default: open-control-bridge)
    /// - macOS: launchd label (default: com.petitechose.open-control-bridge)
    #[arg(long)]
    bridge_service_id: Option<String>,

    /// Local oc-bridge control port (pause/resume IPC).
    #[arg(long, default_value_t = 7999)]
    bridge_control_port: u16,

    /// Max time to wait for oc-bridge IPC.
    #[arg(long, default_value_t = 2500)]
    bridge_control_timeout_ms: u64,

    /// Emit JSON output.
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

    #[command(flatten)]
    bridge: BridgeControlArgs,

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
        Command::Doctor(args) => cmd_doctor(args),
    };

    process::exit(exit_code);
}

fn cmd_doctor(args: DoctorArgs) -> i32 {
    let service_id = args
        .bridge_service_id
        .clone()
        .unwrap_or_else(midi_studio_loader::bridge_control::default_service_id_for_platform);

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

    let svc_status = midi_studio_loader::bridge_control::service_status(&service_id);
    let procs = midi_studio_loader::bridge_control::list_oc_bridge_processes();

    let control_timeout = Duration::from_millis(args.bridge_control_timeout_ms);
    let control = if args.no_bridge_control {
        None
    } else {
        Some(midi_studio_loader::bridge_control::control_status(
            args.bridge_control_port,
            control_timeout,
        ))
    };

    if args.json {
        let targets_val = serde_json::Value::Array(
            targets
                .iter()
                .enumerate()
                .map(|(i, t)| {
                    let mut v = serde_json::to_value(t)
                        .unwrap_or_else(|_| serde_json::Value::Object(serde_json::Map::new()));
                    if let serde_json::Value::Object(obj) = &mut v {
                        obj.insert("index".to_string(), serde_json::Value::from(i as u64));
                        obj.insert("id".to_string(), serde_json::Value::from(t.id()));
                    }
                    v
                })
                .collect(),
        );

        let mut ev = JsonEvent::status("doctor")
            .with_str("service_id", &service_id)
            .with_value("targets", targets_val)
            .with_value(
                "processes",
                serde_json::to_value(&procs)
                    .unwrap_or_else(|_| serde_json::Value::Array(Vec::new())),
            );

        ev = match &control {
            None => ev.with_u64("control_checked", 0),
            Some(Ok(st)) => ev.with_u64("control_checked", 1).with_value(
                "control",
                serde_json::to_value(st)
                    .unwrap_or_else(|_| serde_json::Value::Object(serde_json::Map::new())),
            ),
            Some(Err(e)) => ev
                .with_u64("control_checked", 1)
                .with_str("control_error", &e.to_string()),
        };

        ev = match svc_status {
            Ok(s) => ev.with_value(
                "service_status",
                serde_json::to_value(s).unwrap_or_else(|_| serde_json::Value::from("unknown")),
            ),
            Err(e) => ev.with_str("service_error", &e.to_string()),
        };

        emit_json(&ev);
        return EXIT_OK;
    }

    eprintln!("midi-studio-loader doctor");
    eprintln!("targets: {}", targets.len());
    for (i, t) in targets.iter().enumerate() {
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

    eprintln!(
        "oc-bridge control: 127.0.0.1:{} (timeout {}ms){}",
        args.bridge_control_port,
        args.bridge_control_timeout_ms,
        if args.no_bridge_control {
            " (skipped)"
        } else {
            ""
        }
    );
    if let Some(control) = control {
        match control {
            Ok(st) => {
                eprintln!(
                    "  ok={} paused={} serial_open={:?}",
                    st.ok, st.paused, st.serial_open
                );
                if let Some(m) = st.message {
                    eprintln!("  message: {m}");
                }
            }
            Err(e) => eprintln!("  error: {e}"),
        }
    }

    eprintln!("oc-bridge service: {service_id}");
    match svc_status {
        Ok(s) => eprintln!("  status: {s:?}"),
        Err(e) => eprintln!("  error: {e}"),
    }

    eprintln!("oc-bridge processes: {}", procs.len());
    for p in procs {
        eprintln!(
            "  pid={} restartable={} exe={}",
            p.pid,
            if p.restartable { "yes" } else { "no" },
            p.exe.as_deref().unwrap_or("")
        );
    }

    EXIT_OK
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
            emit_json(&JsonEvent::status("bridge_pause_start"));
        } else if args.verbose {
            eprintln!("pausing oc-bridge...");
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
                    emit_json(
                        &JsonEvent::status("bridge_paused")
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
                } else if args.verbose {
                    eprintln!("oc-bridge paused ({:?})", info.method);
                }
            }
            midi_studio_loader::bridge_control::BridgePauseOutcome::Skipped(_) => {
                if args.verbose {
                    eprintln!("oc-bridge pause skipped");
                }
            }
            midi_studio_loader::bridge_control::BridgePauseOutcome::Failed(e) => {
                if args.verbose {
                    eprintln!("oc-bridge pause failed: {}", e.message);
                    if let Some(hint) = &e.hint {
                        eprintln!("hint: {hint}");
                    }
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

    let exit_code = if any_ambiguous {
        EXIT_AMBIGUOUS
    } else if any_failed {
        EXIT_NO_DEVICE
    } else {
        EXIT_OK
    };

    if let Some(mut g) = bridge_guard {
        if args.json {
            emit_json(&JsonEvent::status("bridge_resume_start"));
        } else if args.verbose {
            eprintln!("resuming oc-bridge...");
        }

        let hint = g.resume_hint();
        match g.resume() {
            Ok(()) => {
                if args.json {
                    emit_json(&JsonEvent::status("bridge_resumed"));
                } else if args.verbose {
                    eprintln!("oc-bridge resumed");
                }
            }
            Err(e) => {
                if args.json {
                    let mut ev = JsonEvent::status("bridge_resume_failed")
                        .with_str("message", &e.to_string());
                    if let Some(hint) = hint {
                        ev = ev.with_str("hint", &hint);
                    }
                    emit_json(&ev);
                } else if args.verbose {
                    eprintln!("oc-bridge resume failed: {e}");
                }
            }
        }
    }

    exit_code
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
        let r = api::plan_teensy41_with_selection(&args.hex, &opts, selection, |ev| {
            handle_flash_event(&args, ev)
        });
        return match r {
            Ok(plan) => {
                if args.json {
                    emit_json(
                        &JsonEvent::status("dry_run")
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
                } else {
                    eprintln!("Dry run OK");
                    eprintln!(
                        "Firmware: {} bytes, blocks_to_write={}/{}",
                        plan.firmware.byte_count,
                        plan.firmware.blocks_to_write.len(),
                        plan.firmware.num_blocks
                    );
                    eprintln!("Targets: {}", plan.selected_targets.len());
                    for t in &plan.selected_targets {
                        eprintln!("- {}", t.id());
                    }
                    if plan.needs_serial && opts.bridge.enabled {
                        eprintln!(
                            "Bridge: would pause/resume oc-bridge (control port {})",
                            opts.bridge.control_port
                        );
                    }
                }

                EXIT_OK
            }
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
        };
    }

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
        api::FlashEvent::BridgePauseStart => {
            if args.json {
                emit_json(&JsonEvent::status("bridge_pause_start"));
            } else if args.verbose {
                eprintln!("pausing oc-bridge...");
            }
        }
        api::FlashEvent::BridgePaused { info } => {
            if args.json {
                let method = match info.method {
                    midi_studio_loader::bridge_control::BridgePauseMethod::Control => "control",
                    midi_studio_loader::bridge_control::BridgePauseMethod::Service => "service",
                    midi_studio_loader::bridge_control::BridgePauseMethod::Process => "process",
                };
                emit_json(
                    &JsonEvent::status("bridge_paused")
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
            } else if args.verbose {
                eprintln!("oc-bridge paused ({:?})", info.method);
            }
        }
        api::FlashEvent::BridgePauseSkipped { reason } => {
            if args.json {
                let reason = match reason {
                    midi_studio_loader::bridge_control::BridgePauseSkipReason::Disabled => {
                        "disabled"
                    }
                    midi_studio_loader::bridge_control::BridgePauseSkipReason::NotRunning => {
                        "not_running"
                    }
                    midi_studio_loader::bridge_control::BridgePauseSkipReason::NotInstalled => {
                        "not_installed"
                    }
                    midi_studio_loader::bridge_control::BridgePauseSkipReason::ProcessNotRestartable => {
                        "process_not_restartable"
                    }
                };
                emit_json(&JsonEvent::status("bridge_pause_skipped").with_str("reason", reason));
            } else if args.verbose {
                eprintln!("oc-bridge pause skipped");
            }
        }
        api::FlashEvent::BridgePauseFailed { error } => {
            if args.json {
                let mut ev =
                    JsonEvent::status("bridge_pause_failed").with_str("message", &error.message);
                if let Some(hint) = &error.hint {
                    ev = ev.with_str("hint", hint);
                }
                emit_json(&ev);
            } else if args.verbose {
                eprintln!("oc-bridge pause failed: {}", error.message);
                if let Some(hint) = &error.hint {
                    eprintln!("hint: {hint}");
                }
            }
        }
        api::FlashEvent::BridgeResumeStart => {
            if args.json {
                emit_json(&JsonEvent::status("bridge_resume_start"));
            } else if args.verbose {
                eprintln!("resuming oc-bridge...");
            }
        }
        api::FlashEvent::BridgeResumed => {
            if args.json {
                emit_json(&JsonEvent::status("bridge_resumed"));
            } else if args.verbose {
                eprintln!("oc-bridge resumed");
            }
        }
        api::FlashEvent::BridgeResumeFailed { error } => {
            if args.json {
                let mut ev =
                    JsonEvent::status("bridge_resume_failed").with_str("message", &error.message);
                if let Some(hint) = &error.hint {
                    ev = ev.with_str("hint", hint);
                }
                emit_json(&ev);
            } else if args.verbose {
                eprintln!("oc-bridge resume failed: {}", error.message);
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

    fn with_value(mut self, k: &'static str, v: serde_json::Value) -> Self {
        self.fields.insert(k, v);
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
