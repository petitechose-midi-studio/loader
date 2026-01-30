use std::collections::BTreeMap;

use midi_studio_loader::{api, targets};

use crate::output::{target_to_value, DoctorReport, DryRunSummary, Event, OutputOptions, Reporter};

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
}

impl JsonOutput {
    pub fn new(opts: OutputOptions) -> Self {
        Self { opts }
    }
}

impl JsonOutput {
    fn json_value(&mut self, value: serde_json::Value) {
        println!(
            "{}",
            serde_json::to_string(&value).unwrap_or_else(|_| "{}".to_string())
        );
    }

    fn json_event(&mut self, ev: JsonEvent) {
        println!(
            "{}",
            serde_json::to_string(&ev).unwrap_or_else(|_| "{}".to_string())
        );
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
            Event::Flash(ev) => self.json_event(flash_event_to_json(ev)),
            Event::DryRun(summary) => emit_dry_run(summary, self),
            Event::ListTargets(targets) => {
                for (i, t) in targets.iter().enumerate() {
                    self.json_value(target_to_value(i, t));
                }
            }
            Event::Doctor(report) => emit_doctor(report, self),
            Event::Error { code, message } => self.error_event(code, &message),
            Event::HintAmbiguousTargets => {}
        }
    }

    fn finish(&mut self) {}
}

fn emit_dry_run(summary: DryRunSummary, out: &mut JsonOutput) {
    out.json_event(
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
            ),
    );
}

fn emit_doctor(report: DoctorReport, out: &mut JsonOutput) {
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

    out.json_event(ev);
}

pub fn flash_event_to_json(ev: api::FlashEvent) -> JsonEvent {
    match ev {
        api::FlashEvent::DiscoverStart => JsonEvent::status("discover_start"),
        api::FlashEvent::TargetDetected { index, target } => JsonEvent::status("target_detected")
            .with_u64("index", index as u64)
            .with_str("target_id", &target.id())
            .with_str(
                "kind",
                match target.kind() {
                    targets::TargetKind::HalfKay => "halfkay",
                    targets::TargetKind::Serial => "serial",
                },
            ),
        api::FlashEvent::DiscoverDone { count } => {
            JsonEvent::status("discover_done").with_u64("count", count as u64)
        }
        api::FlashEvent::TargetSelected { target_id } => {
            JsonEvent::status("target_selected").with_str("target_id", &target_id)
        }
        api::FlashEvent::BridgePauseStart => JsonEvent::status("bridge_pause_start"),
        api::FlashEvent::BridgePaused { info } => {
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
        api::FlashEvent::BridgePauseSkipped { reason } => {
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
        api::FlashEvent::BridgePauseFailed { error } => {
            let mut ev =
                JsonEvent::status("bridge_pause_failed").with_str("message", &error.message);
            if let Some(hint) = &error.hint {
                ev = ev.with_str("hint", hint);
            }
            ev
        }
        api::FlashEvent::BridgeResumeStart => JsonEvent::status("bridge_resume_start"),
        api::FlashEvent::BridgeResumed => JsonEvent::status("bridge_resumed"),
        api::FlashEvent::BridgeResumeFailed { error } => {
            let mut ev =
                JsonEvent::status("bridge_resume_failed").with_str("message", &error.message);
            if let Some(hint) = &error.hint {
                ev = ev.with_str("hint", hint);
            }
            ev
        }
        api::FlashEvent::HexLoaded { bytes, blocks } => JsonEvent::status("hex_loaded")
            .with_u64("bytes", bytes as u64)
            .with_u64("blocks", blocks as u64),
        api::FlashEvent::TargetStart { target_id, kind } => JsonEvent::status("target_start")
            .with_str("target_id", &target_id)
            .with_str(
                "kind",
                match kind {
                    targets::TargetKind::HalfKay => "halfkay",
                    targets::TargetKind::Serial => "serial",
                },
            ),
        api::FlashEvent::TargetDone {
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
        api::FlashEvent::SoftReboot { target_id, port } => JsonEvent::status("soft_reboot")
            .with_str("target_id", &target_id)
            .with_str("port", &port),
        api::FlashEvent::SoftRebootSkipped { target_id, error } => {
            JsonEvent::status("soft_reboot_skipped")
                .with_str("target_id", &target_id)
                .with_str("message", &error)
        }
        api::FlashEvent::HalfKayAppeared { target_id, path } => {
            JsonEvent::status("halfkay_appeared")
                .with_str("target_id", &target_id)
                .with_str("path", &path)
        }
        api::FlashEvent::HalfKayOpen { target_id, path } => JsonEvent::status("halfkay_open")
            .with_str("target_id", &target_id)
            .with_str("path", &path),
        api::FlashEvent::Block {
            target_id,
            index,
            total,
            addr,
        } => JsonEvent::status("block")
            .with_str("target_id", &target_id)
            .with_u64("i", index as u64)
            .with_u64("n", total as u64)
            .with_u64("addr", addr as u64),
        api::FlashEvent::Retry {
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
        api::FlashEvent::Boot { target_id } => {
            JsonEvent::status("boot").with_str("target_id", &target_id)
        }
        api::FlashEvent::Done { target_id } => {
            JsonEvent::status("done").with_str("target_id", &target_id)
        }
    }
}
