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
    DryRun(DryRunSummary),
    ListTargets(Vec<targets::Target>),
    Doctor(DoctorReport),
    Error { code: i32, message: String },
    HintAmbiguousTargets,
}

pub trait Reporter {
    fn emit(&mut self, event: Event);
    fn finish(&mut self);
}

pub fn make_for_flash(args: &cli::FlashArgs) -> Box<dyn Reporter> {
    let opts = OutputOptions {
        verbose: args.verbose,
        quiet: args.quiet,
        json_timestamps: args.json_timestamps,
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
        obj.insert("id".to_string(), serde_json::Value::from(t.id()));
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
