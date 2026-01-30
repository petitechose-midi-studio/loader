use std::io::{IsTerminal, Write};

use midi_studio_loader::{api, targets};

use midi_studio_loader::teensy41;

use crate::output::{
    format_target_line, DoctorReport, DryRunSummary, Event, OutputOptions, Reporter,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Quiet,
    Verbose,
    Progress,
}

pub struct HumanOutput {
    opts: OutputOptions,
    is_tty: bool,
    wait_enabled: bool,
    waiting_printed: bool,
    progress_active: bool,
    last_percent: Option<u64>,
    detected: Vec<Option<targets::Target>>,
}

impl HumanOutput {
    pub fn new(opts: OutputOptions) -> Self {
        Self {
            opts,
            is_tty: std::io::stderr().is_terminal(),
            wait_enabled: false,
            waiting_printed: false,
            progress_active: false,
            last_percent: None,
            detected: Vec::new(),
        }
    }

    pub fn with_wait(mut self, wait: bool) -> Self {
        self.wait_enabled = wait;
        self
    }

    fn mode(&self) -> Mode {
        if self.opts.quiet {
            Mode::Quiet
        } else if self.opts.verbose {
            Mode::Verbose
        } else {
            Mode::Progress
        }
    }

    fn remember_target(&mut self, index: usize, target: targets::Target) {
        if self.detected.len() <= index {
            self.detected.resize_with(index + 1, || None);
        }
        self.detected[index] = Some(target);
    }

    fn finish_line(&mut self) {
        if self.progress_active {
            eprintln!();
            self.progress_active = false;
        }
    }

    fn println(&mut self, msg: &str) {
        if self.mode() == Mode::Quiet {
            return;
        }
        self.finish_line();
        eprintln!("{msg}");
    }

    fn progress_update(&mut self, percent: u64, i: usize, n: usize, addr: usize) {
        if self.mode() != Mode::Progress {
            return;
        }

        if self.is_tty {
            eprint!("\r  programming {percent:3}% ({i}/{n}) @ 0x{addr:06X}");
            let _ = std::io::stderr().flush();
            self.progress_active = true;
            self.last_percent = Some(percent);
            return;
        }

        let last = self.last_percent.unwrap_or(0);
        if percent == 0 || percent == 100 || percent >= last + 10 {
            self.last_percent = Some(percent);
            self.println(&format!("  programming {percent:3}% ({i}/{n})"));
        }
    }

    pub(crate) fn ambiguous_help_lines(detected: &[Option<targets::Target>]) -> Vec<String> {
        detected
            .iter()
            .enumerate()
            .filter_map(|(i, t)| t.as_ref().map(|t| format_target_line(i, t)))
            .collect()
    }

    fn print_ambiguous_help(&mut self) {
        if self.mode() == Mode::Quiet {
            return;
        }

        let lines = Self::ambiguous_help_lines(&self.detected);
        if lines.is_empty() {
            return;
        }

        self.println("");
        self.println("Detected targets:");
        for line in lines {
            self.println(&line);
        }
        self.println("\nHint: use --device index:<n> (e.g. index:0), or --all, or run `midi-studio-loader list`.");
    }
}

impl HumanOutput {
    fn on_flash_event(&mut self, ev: api::FlashEvent) {
        match ev {
            api::FlashEvent::DiscoverStart => {
                if self.mode() != Mode::Quiet {
                    self.println("discover targets...");
                }
            }
            api::FlashEvent::TargetDetected { index, target } => {
                let id = target.id();
                self.remember_target(index, target);
                if self.mode() == Mode::Verbose {
                    self.println(&format!("target[{index}]: {id}"));
                }
            }
            api::FlashEvent::DiscoverDone { count } => {
                if self.mode() == Mode::Progress {
                    if count == 0 && self.wait_enabled && !self.waiting_printed {
                        self.println("waiting for device... (use --wait-timeout-ms to limit)");
                        self.waiting_printed = true;
                    }
                    if count > 0 {
                        self.println(&format!("found {count} target(s)"));
                    }
                }
            }
            api::FlashEvent::TargetSelected { target_id } => {
                if self.mode() != Mode::Quiet {
                    self.println(&format!("selected: {target_id}"));
                }
            }
            api::FlashEvent::BridgePauseStart => {
                if self.mode() != Mode::Quiet {
                    self.println("pausing oc-bridge...");
                }
            }
            api::FlashEvent::BridgePaused { info } => {
                if self.mode() != Mode::Quiet {
                    self.println(&format!("oc-bridge paused ({:?})", info.method));
                }
            }
            api::FlashEvent::BridgePauseSkipped { reason } => {
                if self.mode() == Mode::Verbose {
                    self.println(&format!("oc-bridge pause skipped ({reason:?})"));
                }
            }
            api::FlashEvent::BridgePauseFailed { error } => {
                if self.mode() != Mode::Quiet {
                    self.println(&format!("oc-bridge pause failed: {}", error.message));
                }
            }
            api::FlashEvent::BridgeResumeStart => {
                if self.mode() == Mode::Verbose {
                    self.println("resuming oc-bridge...");
                }
            }
            api::FlashEvent::BridgeResumed => {
                if self.mode() == Mode::Verbose {
                    self.println("oc-bridge resumed");
                }
            }
            api::FlashEvent::BridgeResumeFailed { error } => {
                if self.mode() == Mode::Verbose {
                    self.println(&format!("oc-bridge resume failed: {}", error.message));
                }
            }
            api::FlashEvent::HexLoaded { bytes, blocks } => {
                if self.mode() == Mode::Verbose {
                    self.println(&format!(
                        "Loaded {bytes} bytes ({blocks} blocks) for Teensy 4.1"
                    ));
                } else if self.mode() == Mode::Progress {
                    self.println(&format!("firmware loaded: {bytes} bytes ({blocks} blocks)"));
                }
            }
            api::FlashEvent::TargetStart { target_id, .. } => {
                if self.mode() == Mode::Verbose {
                    self.println(&format!("target start: {target_id}"));
                } else if self.mode() == Mode::Progress {
                    self.println(&format!("target: {target_id}"));
                    self.last_percent = None;
                }
            }
            api::FlashEvent::TargetDone {
                target_id,
                ok,
                message,
            } => {
                if self.mode() == Mode::Verbose {
                    if ok {
                        self.println(&format!("target done: {target_id}"));
                    } else {
                        self.println(&format!(
                            "target failed: {target_id}: {}",
                            message.unwrap_or_default()
                        ));
                    }
                } else if self.mode() == Mode::Progress {
                    self.finish_line();
                    if ok {
                        self.println(&format!("ok: {target_id}"));
                    } else {
                        self.println(&format!(
                            "failed: {target_id}: {}",
                            message.unwrap_or_default()
                        ));
                    }
                }
            }
            api::FlashEvent::SoftReboot { port, .. } => {
                if self.mode() == Mode::Verbose {
                    self.println(&format!("Soft reboot via serial: {port} (baud=134)"));
                } else if self.mode() == Mode::Progress {
                    self.println(&format!("soft reboot: {port}"));
                }
            }
            api::FlashEvent::SoftRebootSkipped { error, .. } => {
                if self.mode() != Mode::Quiet {
                    self.println(&format!("soft reboot skipped: {error}"));
                }
            }
            api::FlashEvent::HalfKayAppeared { .. } => {
                if self.mode() != Mode::Quiet {
                    self.println("halfkay appeared");
                }
            }
            api::FlashEvent::HalfKayOpen { path, .. } => {
                if self.mode() == Mode::Verbose {
                    self.println(&format!("HalfKay open: {path}"));
                } else if self.mode() == Mode::Progress {
                    self.println("halfkay open");
                }
            }
            api::FlashEvent::Block {
                index, total, addr, ..
            } => {
                if self.mode() == Mode::Verbose {
                    self.println(&format!(
                        "program block {}/{} @ 0x{addr:06X}",
                        index + 1,
                        total
                    ));
                } else if self.mode() == Mode::Progress {
                    let percent = ((index + 1) as u64 * 100).saturating_div(total.max(1) as u64);
                    self.progress_update(percent, index + 1, total, addr);
                }
            }
            api::FlashEvent::Retry {
                addr,
                attempt,
                retries,
                error,
                ..
            } => {
                if self.mode() != Mode::Quiet {
                    self.finish_line();
                    self.println(&format!(
                        "retry write at 0x{addr:06X} ({attempt}/{retries}): {error}"
                    ));
                }
            }
            api::FlashEvent::Boot { .. } => {
                if self.mode() == Mode::Progress {
                    self.finish_line();
                    self.println("booting device...");
                }
            }
            api::FlashEvent::Done { .. } => {
                if self.mode() == Mode::Progress {
                    self.finish_line();
                }
            }
        }
    }
}

impl Reporter for HumanOutput {
    fn emit(&mut self, event: Event) {
        match event {
            Event::Flash(ev) => self.on_flash_event(ev),
            Event::DryRun(summary) => emit_dry_run(summary, self),
            Event::ListTargets(targets) => emit_list_targets(&targets, self),
            Event::Doctor(report) => emit_doctor(report, self),
            Event::Error { code: _, message } => {
                self.finish_line();
                eprintln!("error: {message}");
            }
            Event::HintAmbiguousTargets => self.print_ambiguous_help(),
        }
    }

    fn finish(&mut self) {
        self.finish_line();
    }
}

fn emit_list_targets(targets: &[targets::Target], out: &mut HumanOutput) {
    if targets.is_empty() {
        out.println(&format!(
            "No targets found (HalfKay {:04X}:{:04X} or PJRC USB serial)",
            teensy41::VID,
            teensy41::PID_HALFKAY
        ));
        return;
    }

    for (i, t) in targets.iter().enumerate() {
        out.println(&format_target_line(i, t));
    }
}

fn emit_doctor(report: DoctorReport, out: &mut HumanOutput) {
    out.println("midi-studio-loader doctor");
    out.println(&format!("targets: {}", report.targets.len()));
    for (i, t) in report.targets.iter().enumerate() {
        out.println(&format_target_line(i, t));
    }

    out.println(&format!(
        "oc-bridge control: 127.0.0.1:{} (timeout {}ms){}",
        report.control_port,
        report.control_timeout_ms,
        if report.control_checked {
            ""
        } else {
            " (skipped)"
        }
    ));

    if report.control_checked {
        if let Some(st) = report.control {
            out.println(&format!(
                "  ok={} paused={} serial_open={:?}",
                st.ok, st.paused, st.serial_open
            ));
            if let Some(m) = st.message {
                out.println(&format!("  message: {m}"));
            }
        } else if let Some(e) = report.control_error {
            out.println(&format!("  error: {e}"));
        }
    }

    out.println(&format!("oc-bridge service: {}", report.service_id));
    match (report.service_status, report.service_error) {
        (Some(s), _) => out.println(&format!("  status: {s:?}")),
        (None, Some(e)) => out.println(&format!("  error: {e}")),
        _ => {}
    }

    out.println(&format!("oc-bridge processes: {}", report.processes.len()));
    for p in report.processes {
        out.println(&format!(
            "  pid={} restartable={} exe={}",
            p.pid,
            if p.restartable { "yes" } else { "no" },
            p.exe.as_deref().unwrap_or("")
        ));
    }
}

fn emit_dry_run(summary: DryRunSummary, out: &mut HumanOutput) {
    if out.mode() == Mode::Quiet {
        return;
    }

    out.println("Dry run OK");
    out.println(&format!(
        "Firmware: {} bytes, blocks_to_write={}/{},",
        summary.bytes, summary.blocks_to_write, summary.blocks
    ));
    out.println(&format!("Targets: {}", summary.target_ids.len()));
    for id in &summary.target_ids {
        out.println(&format!("- {id}"));
    }
    if summary.needs_serial && summary.bridge_enabled {
        out.println(&format!(
            "Bridge: would pause/resume oc-bridge (control port {})",
            summary.bridge_control_port
        ));
    }
}
