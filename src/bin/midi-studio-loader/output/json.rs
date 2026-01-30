use std::collections::BTreeMap;
use std::time::Instant;

use midi_studio_loader::{operation::OperationEvent, targets};

use crate::output::{
    target_to_value, DoctorReport, DryRunSummary, Event, JsonProgressMode, OperationSummary,
    OutputOptions, Reporter,
};

#[derive(serde::Serialize)]
pub struct JsonEvent {
    schema: u32,
    event: &'static str,
    #[serde(flatten)]
    fields: BTreeMap<&'static str, serde_json::Value>,
}

impl JsonEvent {
    pub fn status(event: &'static str) -> Self {
        Self {
            schema: 1,
            event,
            fields: BTreeMap::new(),
        }
    }

    pub fn with_u64(mut self, k: &'static str, v: u64) -> Self {
        self.fields.insert(k, serde_json::Value::from(v));
        self
    }

    pub fn with_str(mut self, k: &'static str, v: &str) -> Self {
        self.fields.insert(k, serde_json::Value::from(v));
        self
    }

    pub fn with_value(mut self, k: &'static str, v: serde_json::Value) -> Self {
        self.fields.insert(k, v);
        self
    }
}

pub struct JsonOutput {
    opts: OutputOptions,
    start: Instant,
    last_percent: Option<u64>,
}

impl JsonOutput {
    pub fn new(opts: OutputOptions) -> Self {
        Self {
            opts,
            start: Instant::now(),
            last_percent: None,
        }
    }
}

impl JsonOutput {
    pub(crate) fn render_event_json(&mut self, ev: JsonEvent) -> String {
        let mut ev = ev;
        if self.opts.json_timestamps {
            ev.fields.insert(
                "t_ms",
                serde_json::Value::from(self.start.elapsed().as_millis() as u64),
            );
        }
        serde_json::to_string(&ev).unwrap_or_else(|_| "{}".to_string())
    }

    fn json_event(&mut self, ev: JsonEvent) {
        println!("{}", self.render_event_json(ev));
    }

    fn error_event(&mut self, code: i32, msg: &str) {
        self.json_event(
            JsonEvent::status("error")
                .with_u64("code", code as u64)
                .with_str("message", msg),
        );

        if self.opts.verbose {
            eprintln!("error: {msg}");
        }
    }
}

impl Reporter for JsonOutput {
    fn emit(&mut self, event: Event) {
        match event {
            Event::Operation(ev) => self.emit_operation(ev),
            Event::OperationSummary(summary) => {
                self.json_event(operation_summary_to_json(summary));
            }
            Event::DryRun(summary) => self.json_event(dry_run_to_json(summary)),
            Event::ListTargets(targets) => self.json_event(list_to_json(&targets)),
            Event::Doctor(report) => self.json_event(doctor_to_json(report)),
            Event::Error { code, message } => self.error_event(code, &message),
            Event::HintAmbiguousTargets => {}
        }
    }

    fn finish(&mut self) {}
}

pub fn list_to_json(targets: &[targets::Target]) -> JsonEvent {
    JsonEvent::status("list")
        .with_u64("count", targets.len() as u64)
        .with_value(
            "targets",
            serde_json::Value::Array(
                targets
                    .iter()
                    .enumerate()
                    .map(|(i, t)| target_to_value(i, t))
                    .collect(),
            ),
        )
}

impl JsonOutput {
    fn emit_operation(&mut self, ev: OperationEvent) {
        match &ev {
            OperationEvent::TargetStart { .. } => {
                self.last_percent = None;
            }
            OperationEvent::Block { index, total, .. } => match self.opts.json_progress {
                JsonProgressMode::Blocks => {}
                JsonProgressMode::None => return,
                JsonProgressMode::Percent => {
                    let total_u64 = (*total).max(1) as u64;
                    let percent = ((*index + 1) as u64).saturating_mul(100) / total_u64;
                    let should_emit = *index == 0
                        || *index + 1 == *total
                        || self.last_percent.map(|p| p != percent).unwrap_or(true);
                    if !should_emit {
                        return;
                    }
                    self.last_percent = Some(percent);
                }
            },
            _ => {}
        }

        self.json_event(operation_event_to_json(ev));
    }
}

pub fn dry_run_to_json(summary: DryRunSummary) -> JsonEvent {
    JsonEvent::status("dry_run")
        .with_u64("bytes", summary.bytes as u64)
        .with_u64("blocks", summary.blocks as u64)
        .with_u64("blocks_to_write", summary.blocks_to_write as u64)
        .with_u64("targets", summary.target_ids.len() as u64)
        .with_u64("needs_serial", if summary.needs_serial { 1 } else { 0 })
        .with_u64("bridge_enabled", if summary.bridge_enabled { 1 } else { 0 })
        .with_u64("bridge_control_port", summary.bridge_control_port as u64)
        .with_value(
            "target_ids",
            serde_json::Value::Array(
                summary
                    .target_ids
                    .iter()
                    .map(|t| serde_json::Value::from(t.clone()))
                    .collect(),
            ),
        )
}

pub fn operation_summary_to_json(summary: OperationSummary) -> JsonEvent {
    let OperationSummary {
        operation,
        exit_code,
        message,
        targets_ok,
        targets_failed,
        blocks,
        retries,
        bridge_pause,
        bridge_method,
        bridge_reason,
    } = summary;

    let total = targets_ok.len() + targets_failed.len();

    let mut ev = JsonEvent::status("operation_summary")
        .with_str("operation", operation)
        .with_u64("ok", if exit_code == 0 { 1 } else { 0 })
        .with_u64("exit_code", exit_code.max(0) as u64)
        .with_u64("targets_total", total as u64)
        .with_u64("targets_ok", targets_ok.len() as u64)
        .with_u64("targets_failed", targets_failed.len() as u64)
        .with_u64("blocks", blocks)
        .with_u64("retries", retries)
        .with_str("bridge_pause", &bridge_pause)
        .with_value(
            "targets_ok_ids",
            serde_json::Value::Array(targets_ok.into_iter().map(Into::into).collect()),
        )
        .with_value(
            "targets_failed_ids",
            serde_json::Value::Array(targets_failed.into_iter().map(Into::into).collect()),
        );

    if let Some(m) = &bridge_method {
        ev = ev.with_str("bridge_method", m);
    }
    if let Some(r) = &bridge_reason {
        ev = ev.with_str("bridge_reason", r);
    }
    if let Some(msg) = &message {
        ev = ev.with_str("message", msg);
    }

    ev
}

pub fn doctor_to_json(report: DoctorReport) -> JsonEvent {
    let targets_val = serde_json::Value::Array(
        report
            .targets
            .iter()
            .enumerate()
            .map(|(i, t)| target_to_value(i, t))
            .collect(),
    );

    let mut ev = JsonEvent::status("doctor")
        .with_str("service_id", &report.service_id)
        .with_value("targets", targets_val)
        .with_value(
            "processes",
            serde_json::to_value(&report.processes)
                .unwrap_or_else(|_| serde_json::Value::Array(Vec::new())),
        )
        .with_u64("control_port", report.control_port as u64)
        .with_u64("control_timeout_ms", report.control_timeout_ms)
        .with_u64(
            "control_checked",
            if report.control_checked { 1 } else { 0 },
        );

    if let Some(st) = &report.control {
        ev = ev.with_value(
            "control",
            serde_json::to_value(st)
                .unwrap_or_else(|_| serde_json::Value::Object(serde_json::Map::new())),
        );
    }
    if let Some(e) = &report.control_error {
        ev = ev.with_str("control_error", e);
    }
    if let Some(s) = report.service_status {
        ev = ev.with_value(
            "service_status",
            serde_json::to_value(s).unwrap_or_else(|_| serde_json::Value::from("unknown")),
        );
    }
    if let Some(e) = &report.service_error {
        ev = ev.with_str("service_error", e);
    }

    ev
}

pub fn operation_event_to_json(ev: OperationEvent) -> JsonEvent {
    match ev {
        OperationEvent::DiscoverStart => JsonEvent::status("discover_start"),
        OperationEvent::TargetDetected { index, target } => JsonEvent::status("target_detected")
            .with_value("target", target_to_value(index, &target)),
        OperationEvent::DiscoverDone { count } => {
            JsonEvent::status("discover_done").with_u64("count", count as u64)
        }
        OperationEvent::TargetSelected { target_id } => {
            JsonEvent::status("target_selected").with_str("target_id", &target_id)
        }
        OperationEvent::BridgePauseStart => JsonEvent::status("bridge_pause_start"),
        OperationEvent::BridgePaused { info } => {
            let method = match info.method {
                midi_studio_loader::bridge_control::BridgePauseMethod::Control => "control",
                midi_studio_loader::bridge_control::BridgePauseMethod::Service => "service",
                midi_studio_loader::bridge_control::BridgePauseMethod::Process => "process",
            };
            JsonEvent::status("bridge_paused")
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
                )
        }
        OperationEvent::BridgePauseSkipped { reason } => {
            let reason = match reason {
                midi_studio_loader::bridge_control::BridgePauseSkipReason::Disabled => "disabled",
                midi_studio_loader::bridge_control::BridgePauseSkipReason::NotRunning => "not_running",
                midi_studio_loader::bridge_control::BridgePauseSkipReason::NotInstalled => "not_installed",
                midi_studio_loader::bridge_control::BridgePauseSkipReason::ProcessNotRestartable => {
                    "process_not_restartable"
                }
            };
            JsonEvent::status("bridge_pause_skipped").with_str("reason", reason)
        }
        OperationEvent::BridgePauseFailed { error } => {
            let mut ev =
                JsonEvent::status("bridge_pause_failed").with_str("message", &error.message);
            if let Some(hint) = &error.hint {
                ev = ev.with_str("hint", hint);
            }
            ev
        }
        OperationEvent::BridgeResumeStart => JsonEvent::status("bridge_resume_start"),
        OperationEvent::BridgeResumed => JsonEvent::status("bridge_resumed"),
        OperationEvent::BridgeResumeFailed { error } => {
            let mut ev =
                JsonEvent::status("bridge_resume_failed").with_str("message", &error.message);
            if let Some(hint) = &error.hint {
                ev = ev.with_str("hint", hint);
            }
            ev
        }
        OperationEvent::HexLoaded { bytes, blocks } => JsonEvent::status("hex_loaded")
            .with_u64("bytes", bytes as u64)
            .with_u64("blocks", blocks as u64),
        OperationEvent::TargetStart { target_id, kind } => JsonEvent::status("target_start")
            .with_str("target_id", &target_id)
            .with_str(
                "kind",
                match kind {
                    targets::TargetKind::HalfKay => "halfkay",
                    targets::TargetKind::Serial => "serial",
                },
            ),
        OperationEvent::TargetDone {
            target_id,
            ok,
            message,
        } => {
            let mut ev = JsonEvent::status("target_done")
                .with_str("target_id", &target_id)
                .with_u64("ok", if ok { 1 } else { 0 });
            if let Some(m) = &message {
                ev = ev.with_str("message", m);
            }
            ev
        }
        OperationEvent::SoftReboot { target_id, port } => JsonEvent::status("soft_reboot")
            .with_str("target_id", &target_id)
            .with_str("port", &port),
        OperationEvent::SoftRebootSkipped { target_id, error } => {
            JsonEvent::status("soft_reboot_skipped")
                .with_str("target_id", &target_id)
                .with_str("message", &error)
        }
        OperationEvent::HalfKayAppeared { target_id, path } => {
            JsonEvent::status("halfkay_appeared")
                .with_str("target_id", &target_id)
                .with_str("path", &path)
        }
        OperationEvent::HalfKayOpen { target_id, path } => JsonEvent::status("halfkay_open")
            .with_str("target_id", &target_id)
            .with_str("path", &path),
        OperationEvent::Block {
            target_id,
            index,
            total,
            addr,
        } => JsonEvent::status("block")
            .with_str("target_id", &target_id)
            .with_u64("i", index as u64)
            .with_u64("n", total as u64)
            .with_u64("addr", addr as u64),
        OperationEvent::Retry {
            target_id,
            addr,
            attempt,
            retries,
            error,
        } => JsonEvent::status("retry")
            .with_str("target_id", &target_id)
            .with_u64("addr", addr as u64)
            .with_u64("attempt", attempt as u64)
            .with_u64("retries", retries as u64)
            .with_str("error", &error),
        OperationEvent::Boot { target_id } => {
            JsonEvent::status("boot").with_str("target_id", &target_id)
        }
        OperationEvent::Done { target_id } => {
            JsonEvent::status("done").with_str("target_id", &target_id)
        }
    }
}
