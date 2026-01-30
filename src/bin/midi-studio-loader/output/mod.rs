use crate::cli;

use midi_studio_loader::{bridge_control, operation::OperationEvent, targets};

pub mod human;
pub mod json;

#[cfg(test)]
mod tests;

#[derive(Debug, Clone, Copy)]
pub struct OutputOptions {
    pub verbose: bool,
    pub quiet: bool,
    pub json_timestamps: bool,
    pub json_progress: JsonProgressMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsonProgressMode {
    Blocks,
    Percent,
    None,
}

#[derive(Debug, Clone)]
pub struct DryRunSummary {
    pub bytes: usize,
    pub blocks: usize,
    pub blocks_to_write: usize,
    pub target_ids: Vec<String>,
    pub needs_serial: bool,
    pub bridge_enabled: bool,
    pub bridge_control_port: u16,
}

#[derive(Debug, Clone)]
pub struct DoctorReport {
    pub service_id: String,
    pub targets: Vec<targets::Target>,

    pub control_port: u16,
    pub control_timeout_ms: u64,
    pub control_checked: bool,
    pub control: Option<bridge_control::BridgeControlStatus>,
    pub control_error: Option<String>,

    pub service_status: Option<bridge_control::ServiceStatus>,
    pub service_error: Option<String>,

    pub processes: Vec<bridge_control::OcBridgeProcessInfo>,
}

#[derive(Debug, Clone)]
pub enum Event {
    Operation(OperationEvent),
    OperationSummary(OperationSummary),
    DryRun(DryRunSummary),
    ListTargets(Vec<targets::Target>),
    Doctor(DoctorReport),
    Error { code: i32, message: String },
    HintAmbiguousTargets,
}

#[derive(Debug, Clone)]
pub struct OperationSummary {
    pub operation: &'static str,
    pub exit_code: i32,
    pub message: Option<String>,

    pub targets_ok: Vec<String>,
    pub targets_failed: Vec<String>,

    pub blocks: u64,
    pub retries: u64,

    pub bridge_pause: String,
    pub bridge_method: Option<String>,
    pub bridge_reason: Option<String>,
}

pub struct OperationRecorder {
    operation: &'static str,
    targets_ok: Vec<String>,
    targets_failed: Vec<String>,
    blocks: u64,
    retries: u64,
    bridge_pause: String,
    bridge_method: Option<String>,
    bridge_reason: Option<String>,
}

impl OperationRecorder {
    pub fn new(operation: &'static str) -> Self {
        Self {
            operation,
            targets_ok: Vec::new(),
            targets_failed: Vec::new(),
            blocks: 0,
            retries: 0,
            bridge_pause: "not_attempted".to_string(),
            bridge_method: None,
            bridge_reason: None,
        }
    }

    pub fn observe(&mut self, ev: &OperationEvent) {
        match ev {
            OperationEvent::BridgePauseStart => {
                self.bridge_pause = "attempted".to_string();
            }
            OperationEvent::BridgePaused { info } => {
                self.bridge_pause = "paused".to_string();
                self.bridge_method = Some(
                    match info.method {
                        bridge_control::BridgePauseMethod::Control => "control",
                        bridge_control::BridgePauseMethod::Service => "service",
                        bridge_control::BridgePauseMethod::Process => "process",
                    }
                    .to_string(),
                );
            }
            OperationEvent::BridgePauseSkipped { reason } => {
                self.bridge_pause = "skipped".to_string();
                self.bridge_reason = Some(
                    match reason {
                        bridge_control::BridgePauseSkipReason::Disabled => "disabled",
                        bridge_control::BridgePauseSkipReason::NotRunning => "not_running",
                        bridge_control::BridgePauseSkipReason::NotInstalled => "not_installed",
                        bridge_control::BridgePauseSkipReason::ProcessNotRestartable => {
                            "process_not_restartable"
                        }
                    }
                    .to_string(),
                );
            }
            OperationEvent::BridgePauseFailed { .. } => {
                self.bridge_pause = "failed".to_string();
            }
            OperationEvent::TargetDone { target_id, ok, .. } => {
                if *ok {
                    self.targets_ok.push(target_id.clone());
                } else {
                    self.targets_failed.push(target_id.clone());
                }
            }
            OperationEvent::Block { .. } => {
                self.blocks = self.blocks.saturating_add(1);
            }
            OperationEvent::Retry { .. } => {
                self.retries = self.retries.saturating_add(1);
            }
            _ => {}
        }
    }

    pub fn finish(self, exit_code: i32, message: Option<String>) -> OperationSummary {
        OperationSummary {
            operation: self.operation,
            exit_code,
            message,
            targets_ok: self.targets_ok,
            targets_failed: self.targets_failed,
            blocks: self.blocks,
            retries: self.retries,
            bridge_pause: self.bridge_pause,
            bridge_method: self.bridge_method,
            bridge_reason: self.bridge_reason,
        }
    }
}

pub trait Reporter {
    fn emit(&mut self, event: Event);
    fn finish(&mut self);
}

pub fn make_for_flash(args: &cli::FlashArgs) -> Box<dyn Reporter> {
    let json_progress = match args.json_progress {
        cli::JsonProgressArg::Blocks => JsonProgressMode::Blocks,
        cli::JsonProgressArg::Percent => JsonProgressMode::Percent,
        cli::JsonProgressArg::None => JsonProgressMode::None,
    };
    let opts = OutputOptions {
        verbose: args.verbose,
        quiet: args.quiet,
        json_timestamps: args.json_timestamps,
        json_progress,
    };
    if args.json {
        Box::new(json::JsonOutput::new(opts))
    } else {
        Box::new(human::HumanOutput::new(opts).with_wait(args.wait))
    }
}

pub fn make_for_reboot(args: &cli::RebootArgs) -> Box<dyn Reporter> {
    let opts = OutputOptions {
        verbose: args.verbose,
        quiet: false,
        json_timestamps: args.json_timestamps,
        json_progress: JsonProgressMode::Blocks,
    };
    if args.json {
        Box::new(json::JsonOutput::new(opts))
    } else {
        Box::new(human::HumanOutput::new(opts))
    }
}

pub fn make_for_list(args: &cli::ListArgs) -> Box<dyn Reporter> {
    let opts = OutputOptions {
        verbose: false,
        quiet: false,
        json_timestamps: false,
        json_progress: JsonProgressMode::Blocks,
    };
    if args.json {
        Box::new(json::JsonOutput::new(opts))
    } else {
        Box::new(human::HumanOutput::new(opts))
    }
}

pub fn make_for_doctor(args: &cli::DoctorArgs) -> Box<dyn Reporter> {
    let opts = OutputOptions {
        verbose: false,
        quiet: false,
        json_timestamps: false,
        json_progress: JsonProgressMode::Blocks,
    };
    if args.json {
        Box::new(json::JsonOutput::new(opts))
    } else {
        Box::new(human::HumanOutput::new(opts))
    }
}

pub fn target_to_value(index: usize, t: &targets::Target) -> serde_json::Value {
    let mut v = serde_json::to_value(t)
        .unwrap_or_else(|_| serde_json::Value::Object(serde_json::Map::new()));
    if let serde_json::Value::Object(obj) = &mut v {
        obj.insert("index".to_string(), serde_json::Value::from(index as u64));
        obj.insert("target_id".to_string(), serde_json::Value::from(t.id()));
    }
    v
}

pub fn format_target_line(index: usize, t: &targets::Target) -> String {
    match t {
        targets::Target::HalfKay(hk) => {
            format!("[{index}] halfkay {} {:04X}:{:04X}", t.id(), hk.vid, hk.pid)
        }
        targets::Target::Serial(s) => format!(
            "[{index}] serial  {} {:04X}:{:04X} {}",
            t.id(),
            s.vid,
            s.pid,
            s.product.as_deref().unwrap_or("")
        ),
    }
}
