use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

#[derive(ValueEnum, Clone, Copy, Debug)]
pub enum BridgeMethodArg {
    Auto,
    Control,
    Service,
    Process,
    None,
}

#[derive(ValueEnum, Clone, Copy, Debug)]
pub enum JsonProgressArg {
    /// Emit a JSON event for every written block.
    Blocks,
    /// Emit fewer JSON events by throttling block output to percent changes.
    Percent,
    /// Do not emit per-block progress events.
    None,
}

#[derive(Parser)]
#[command(name = "midi-studio-loader")]
#[command(about = "Teensy 4.1 flasher CLI (HalfKay)")]
#[command(version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Flash an Intel HEX to a Teensy 4.1 in HalfKay bootloader mode.
    Flash(FlashArgs),

    /// Try to enter HalfKay bootloader without the button.
    Reboot(RebootArgs),

    /// List detected targets (HalfKay + PJRC USB serial).
    List(ListArgs),

    /// Diagnose target detection and bridge coordination.
    Doctor(DoctorArgs),
}

#[derive(Parser, Clone)]
pub struct BridgeControlArgs {
    /// Disable automatic oc-bridge pause/resume.
    #[arg(long)]
    pub no_bridge_control: bool,

    /// Bridge pause/resume strategy.
    #[arg(long, value_enum, default_value_t = BridgeMethodArg::Auto)]
    pub bridge_method: BridgeMethodArg,

    /// Disable process fallback when `--bridge-method auto` is used.
    #[arg(long)]
    pub no_process_fallback: bool,

    /// Max time to wait when stopping/starting the bridge.
    #[arg(long, default_value_t = 5000)]
    pub bridge_timeout_ms: u64,

    /// Override the bridge service identifier.
    ///
    /// - Windows: service name (default: OpenControlBridge)
    /// - Linux: systemd user unit (default: open-control-bridge)
    /// - macOS: launchd label (default: com.petitechose.open-control-bridge)
    #[arg(long)]
    pub bridge_service_id: Option<String>,

    /// Local oc-bridge control port (pause/resume IPC).
    #[arg(long, default_value_t = 7999)]
    pub bridge_control_port: u16,

    /// Max time to wait for oc-bridge IPC.
    #[arg(long, default_value_t = 2500)]
    pub bridge_control_timeout_ms: u64,
}

#[derive(Parser)]
pub struct FlashArgs {
    /// Path to Intel HEX firmware.
    pub hex: PathBuf,

    /// Flash every detected target sequentially.
    #[arg(long, conflicts_with = "device")]
    pub all: bool,

    /// Select a specific target (e.g. serial:COM6, halfkay:<path>, index:0).
    #[arg(long, conflicts_with = "all")]
    pub device: Option<String>,

    /// Wait for a target to appear (HalfKay or PJRC USB serial).
    #[arg(long)]
    pub wait: bool,

    /// Max time to wait for device (0 = forever).
    #[arg(long, default_value_t = 0)]
    pub wait_timeout_ms: u64,

    /// Do not reboot after programming.
    #[arg(long)]
    pub no_reboot: bool,

    /// Retries per block on write failure.
    #[arg(long, default_value_t = 3)]
    pub retries: u32,

    /// Prefer a specific serial port name (e.g. COM6) when selecting among multiple devices.
    #[arg(long)]
    pub serial_port: Option<String>,

    #[command(flatten)]
    pub bridge: BridgeControlArgs,

    /// Emit JSON line events to stdout.
    #[arg(long)]
    pub json: bool,

    /// Include monotonic timestamps in JSON events (milliseconds since process start).
    #[arg(long, requires = "json")]
    pub json_timestamps: bool,

    /// JSON progress verbosity.
    ///
    /// - blocks: emit every block (most verbose)
    /// - percent: emit fewer progress events
    /// - none: no per-block progress events
    #[arg(long, value_enum, default_value_t = JsonProgressArg::Percent, requires = "json")]
    pub json_progress: JsonProgressArg,

    /// Validate inputs and selection without flashing.
    #[arg(long)]
    pub dry_run: bool,

    /// Reduce output (only errors).
    #[arg(long, conflicts_with = "verbose")]
    pub quiet: bool,

    /// More logs to stderr.
    #[arg(long, short)]
    pub verbose: bool,
}

#[derive(Parser)]
pub struct ListArgs {
    /// Emit JSON line output.
    #[arg(long)]
    pub json: bool,
}

#[derive(Parser)]
pub struct DoctorArgs {
    /// Skip probing oc-bridge IPC.
    #[arg(long)]
    pub no_bridge_control: bool,

    /// Override the bridge service identifier.
    ///
    /// - Windows: service name (default: OpenControlBridge)
    /// - Linux: systemd user unit (default: open-control-bridge)
    /// - macOS: launchd label (default: com.petitechose.open-control-bridge)
    #[arg(long)]
    pub bridge_service_id: Option<String>,

    /// Local oc-bridge control port (pause/resume IPC).
    #[arg(long, default_value_t = 7999)]
    pub bridge_control_port: u16,

    /// Max time to wait for oc-bridge IPC.
    #[arg(long, default_value_t = 2500)]
    pub bridge_control_timeout_ms: u64,

    /// Emit JSON output.
    #[arg(long)]
    pub json: bool,
}

#[derive(Parser)]
pub struct RebootArgs {
    /// Max time to wait for HalfKay to appear (0 = forever).
    #[arg(long, default_value_t = 60000)]
    pub wait_timeout_ms: u64,

    /// Reboot every detected target sequentially.
    #[arg(long, conflicts_with = "device")]
    pub all: bool,

    /// Select a specific target (e.g. serial:COM6, halfkay:<path>, index:0).
    #[arg(long, conflicts_with = "all")]
    pub device: Option<String>,

    /// Prefer a specific serial port name (e.g. COM6).
    #[arg(long)]
    pub serial_port: Option<String>,

    #[command(flatten)]
    pub bridge: BridgeControlArgs,

    /// Emit JSON line events to stdout.
    #[arg(long)]
    pub json: bool,

    /// Include monotonic timestamps in JSON events (milliseconds since process start).
    #[arg(long, requires = "json")]
    pub json_timestamps: bool,

    /// More logs to stderr.
    #[arg(long, short)]
    pub verbose: bool,
}
