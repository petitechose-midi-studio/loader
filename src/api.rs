use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, Instant};

use thiserror::Error;

use crate::{
    bootloader, bridge_control, halfkay, hex,
    operation::OperationEvent,
    selector, serial_reboot, targets,
    targets::{Target, TargetKind},
};

#[derive(Debug, Clone)]
pub enum FlashSelection {
    Auto,
    All,
    Device(String),
}

#[derive(Debug, Clone)]
pub struct FlashOptions {
    /// Wait for at least one target to be detected.
    pub wait: bool,
    /// Max time to wait when `wait=true` (None = forever).
    pub wait_timeout: Option<Duration>,

    /// Do not reboot after programming.
    pub no_reboot: bool,

    /// Retries per block on write failure.
    pub retries: u32,

    /// Prefer a specific serial port name when selecting among multiple Serial targets.
    ///
    /// Example: "COM6" or "/dev/ttyACM0".
    pub serial_port: Option<String>,

    pub bridge: bridge_control::BridgeControlOptions,

    pub reopen_timeout: Duration,
    pub reopen_delay: Duration,
    pub soft_reboot_delay: Duration,
}

impl Default for FlashOptions {
    fn default() -> Self {
        Self {
            wait: false,
            wait_timeout: None,
            no_reboot: false,
            retries: 3,
            serial_port: None,
            bridge: bridge_control::BridgeControlOptions::default(),
            reopen_timeout: Duration::from_secs(10),
            reopen_delay: Duration::from_millis(150),
            soft_reboot_delay: Duration::from_millis(250),
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum FlashErrorKind {
    NoDevice,
    AmbiguousTarget,
    InvalidHex,
    WriteFailed,
    Unexpected,
}

#[derive(Error, Debug)]
pub enum FlashError {
    #[error("no target device found")]
    NoTargets,

    #[error("ambiguous target: {message}")]
    AmbiguousTarget { message: String },

    #[error("target discovery failed: {source}")]
    DiscoveryFailed {
        #[source]
        source: targets::DiscoverError,
    },

    #[error("invalid HEX: {source}")]
    InvalidHex {
        #[source]
        source: hex::HexError,
    },

    #[error("soft reboot failed on {port}: {source}")]
    SoftRebootFailed {
        port: String,
        #[source]
        source: serial_reboot::SerialRebootError,
    },

    #[error("unable to open HalfKay device at {path}: {source}")]
    OpenHalfKay {
        path: String,
        #[source]
        source: halfkay::HalfKayError,
    },

    #[error("write failed at addr=0x{addr:06X} after {attempts} attempts: {source}")]
    WriteFailed {
        addr: usize,
        attempts: u32,
        #[source]
        source: halfkay::HalfKayError,
    },

    #[error("unable to reopen HalfKay device at {path} while writing addr=0x{addr:06X}: {source}")]
    ReopenFailed {
        path: String,
        addr: usize,
        #[source]
        source: halfkay::HalfKayError,
    },

    #[error("flash failed for {failed}/{total} targets")]
    MultiTargetFailed { failed: usize, total: usize },
}

impl FlashError {
    pub fn kind(&self) -> FlashErrorKind {
        match self {
            FlashError::NoTargets => FlashErrorKind::NoDevice,
            FlashError::AmbiguousTarget { .. } => FlashErrorKind::AmbiguousTarget,
            FlashError::DiscoveryFailed { .. } => FlashErrorKind::Unexpected,
            FlashError::InvalidHex { .. } => FlashErrorKind::InvalidHex,
            FlashError::SoftRebootFailed { .. } => FlashErrorKind::NoDevice,
            FlashError::OpenHalfKay { .. } => FlashErrorKind::NoDevice,
            FlashError::WriteFailed { .. } | FlashError::ReopenFailed { .. } => {
                FlashErrorKind::WriteFailed
            }
            FlashError::MultiTargetFailed { .. } => FlashErrorKind::WriteFailed,
        }
    }
}

pub fn flash_teensy41<F>(
    hex_path: &Path,
    opts: &FlashOptions,
    on_event: F,
) -> Result<(), FlashError>
where
    F: FnMut(OperationEvent),
{
    flash_teensy41_with_selection(hex_path, opts, FlashSelection::Auto, on_event)
}

pub struct FlashPlan {
    pub firmware: hex::FirmwareImage,
    pub selected_targets: Vec<Target>,
    pub needs_serial: bool,
}

pub fn plan_teensy41_with_selection<F>(
    hex_path: &Path,
    opts: &FlashOptions,
    selection: FlashSelection,
    mut on_event: F,
) -> Result<FlashPlan, FlashError>
where
    F: FnMut(OperationEvent),
{
    let fw = hex::FirmwareImage::load_teensy41(hex_path)
        .map_err(|e| FlashError::InvalidHex { source: e })?;

    on_event(OperationEvent::HexLoaded {
        bytes: fw.byte_count,
        blocks: fw.num_blocks,
    });

    let targets = discover_targets_for_flash(opts, &mut on_event)?;
    let selected = select_targets(
        selection,
        opts.serial_port.as_deref(),
        &targets,
        true,
        &mut on_event,
    )?;
    let needs_serial = selected.iter().any(|t| t.kind() == TargetKind::Serial);

    Ok(FlashPlan {
        firmware: fw,
        selected_targets: selected,
        needs_serial,
    })
}

pub fn flash_teensy41_with_selection<F>(
    hex_path: &Path,
    opts: &FlashOptions,
    selection: FlashSelection,
    mut on_event: F,
) -> Result<(), FlashError>
where
    F: FnMut(OperationEvent),
{
    let plan = plan_teensy41_with_selection(hex_path, opts, selection, &mut on_event)?;
    let fw = plan.firmware;
    let selected = plan.selected_targets;
    let needs_serial = plan.needs_serial;
    let mut bridge_guard: Option<bridge_control::BridgeGuard> = None;
    if needs_serial {
        on_event(OperationEvent::BridgePauseStart);
        let paused = bridge_control::pause_oc_bridge(&opts.bridge);
        match &paused.outcome {
            bridge_control::BridgePauseOutcome::Paused(info) => {
                on_event(OperationEvent::BridgePaused { info: info.clone() });
            }
            bridge_control::BridgePauseOutcome::Skipped(reason) => {
                on_event(OperationEvent::BridgePauseSkipped {
                    reason: reason.clone(),
                });
            }
            bridge_control::BridgePauseOutcome::Failed(error) => {
                on_event(OperationEvent::BridgePauseFailed {
                    error: error.clone(),
                });
            }
        }
        bridge_guard = paused.guard;
    }

    let total = selected.len();
    let multi = total > 1;
    let mut failed = 0usize;
    let mut fatal_err: Option<FlashError> = None;

    for target in selected {
        let target_id = target.id();
        on_event(OperationEvent::TargetStart {
            target_id: target_id.clone(),
            kind: target.kind(),
        });

        let r = flash_one_target(&target, &target_id, &fw, opts, &mut on_event);
        match r {
            Ok(()) => {
                on_event(OperationEvent::TargetDone {
                    target_id,
                    ok: true,
                    message: None,
                });
            }
            Err(e) => {
                failed += 1;
                on_event(OperationEvent::TargetDone {
                    target_id: target_id.clone(),
                    ok: false,
                    message: Some(e.to_string()),
                });

                if !multi {
                    fatal_err = Some(e);
                    break;
                }
            }
        }
    }

    let result = if let Some(e) = fatal_err {
        Err(e)
    } else if failed > 0 {
        Err(FlashError::MultiTargetFailed { failed, total })
    } else {
        Ok(())
    };

    if let Some(mut g) = bridge_guard {
        on_event(OperationEvent::BridgeResumeStart);
        let hint = g.resume_hint();
        match g.resume() {
            Ok(()) => on_event(OperationEvent::BridgeResumed),
            Err(e) => on_event(OperationEvent::BridgeResumeFailed {
                error: bridge_control::BridgeControlErrorInfo {
                    message: format!("bridge resume failed: {e}"),
                    hint,
                },
            }),
        }
    }

    result
}

fn discover_targets_for_flash<F>(
    opts: &FlashOptions,
    on_event: &mut F,
) -> Result<Vec<Target>, FlashError>
where
    F: FnMut(OperationEvent),
{
    on_event(OperationEvent::DiscoverStart);

    let start = Instant::now();
    loop {
        let targets =
            targets::discover_targets().map_err(|e| FlashError::DiscoveryFailed { source: e })?;

        for (i, t) in targets.iter().cloned().enumerate() {
            on_event(OperationEvent::TargetDetected {
                index: i,
                target: t,
            });
        }
        on_event(OperationEvent::DiscoverDone {
            count: targets.len(),
        });

        if !targets.is_empty() {
            return Ok(targets);
        }
        if !opts.wait {
            return Err(FlashError::NoTargets);
        }
        if let Some(t) = opts.wait_timeout {
            if start.elapsed() >= t {
                return Err(FlashError::NoTargets);
            }
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

pub(crate) fn select_targets<F>(
    selection: FlashSelection,
    serial_port: Option<&str>,
    targets: &[Target],
    emit_selected_event: bool,
    on_event: &mut F,
) -> Result<Vec<Target>, FlashError>
where
    F: FnMut(OperationEvent),
{
    if targets.is_empty() {
        return Err(FlashError::NoTargets);
    }

    let halfkay: Vec<Target> = targets
        .iter()
        .filter(|t| t.kind() == TargetKind::HalfKay)
        .cloned()
        .collect();

    let serial: Vec<Target> = targets
        .iter()
        .filter(|t| t.kind() == TargetKind::Serial)
        .cloned()
        .collect();

    let selected: Vec<Target> = match selection {
        FlashSelection::All => targets.to_vec(),

        FlashSelection::Device(sel) => {
            let parsed =
                selector::parse_selector(&sel).map_err(|e| FlashError::AmbiguousTarget {
                    message: e.to_string(),
                })?;
            let idx = selector::resolve_one(&parsed, targets).map_err(|e| {
                FlashError::AmbiguousTarget {
                    message: e.to_string(),
                }
            })?;
            vec![targets[idx].clone()]
        }

        FlashSelection::Auto => {
            // Strong default: if exactly one HalfKay device is present, select it even if
            // other serial targets exist (bootloader mode is a strong user signal).
            if halfkay.len() == 1 {
                vec![halfkay[0].clone()]
            } else if !halfkay.is_empty() {
                return Err(FlashError::AmbiguousTarget {
                    message: format!(
                        "multiple HalfKay devices detected ({}); use --device or --all",
                        halfkay.len()
                    ),
                });
            } else if let Some(port) = serial_port {
                let matches: Vec<Target> = serial
                    .iter()
                    .filter_map(|t| match t {
                        Target::Serial(s) if s.port_name == port => Some(t.clone()),
                        _ => None,
                    })
                    .collect();
                if matches.len() == 1 {
                    vec![matches[0].clone()]
                } else if matches.is_empty() {
                    return Err(FlashError::NoTargets);
                } else {
                    return Err(FlashError::AmbiguousTarget {
                        message: format!(
                            "multiple targets matched preferred serial port {port}; use --device"
                        ),
                    });
                }
            } else if targets.len() == 1 {
                vec![targets[0].clone()]
            } else {
                return Err(FlashError::AmbiguousTarget {
                    message: format!(
                        "multiple targets detected ({}); use --device or --all",
                        targets.len()
                    ),
                });
            }
        }
    };

    if emit_selected_event && selected.len() == 1 {
        on_event(OperationEvent::TargetSelected {
            target_id: selected[0].id(),
        });
    }

    Ok(selected)
}

fn flash_one_target<F>(
    target: &Target,
    target_id: &str,
    fw: &hex::FirmwareImage,
    opts: &FlashOptions,
    on_event: &mut F,
) -> Result<(), FlashError>
where
    F: FnMut(OperationEvent),
{
    match target {
        Target::HalfKay(t) => flash_halfkay_path(&t.path, target_id, fw, opts, on_event),
        Target::Serial(t) => {
            // 1) snapshot existing HalfKay devices
            let before = halfkay::list_paths().map_err(|e| FlashError::DiscoveryFailed {
                source: targets::DiscoverError::Hid(e),
            })?;
            let before: HashSet<String> = before.into_iter().collect();

            // 2) reboot selected serial port
            match serial_reboot::soft_reboot_port(&t.port_name) {
                Ok(()) => {
                    on_event(OperationEvent::SoftReboot {
                        target_id: target_id.to_string(),
                        port: t.port_name.clone(),
                    });
                    std::thread::sleep(opts.soft_reboot_delay);
                }
                Err(e) => {
                    on_event(OperationEvent::SoftRebootSkipped {
                        target_id: target_id.to_string(),
                        error: e.to_string(),
                    });
                    return Err(FlashError::SoftRebootFailed {
                        port: t.port_name.clone(),
                        source: e,
                    });
                }
            }

            // 3) wait for a new HalfKay path to appear
            let timeout = opts.wait_timeout.unwrap_or_else(|| Duration::from_secs(60));
            let hk_path =
                bootloader::wait_for_new_halfkay(&before, timeout, Duration::from_millis(50))
                    .map_err(|e| FlashError::AmbiguousTarget {
                        message: e.to_string(),
                    })?;

            on_event(OperationEvent::HalfKayAppeared {
                target_id: target_id.to_string(),
                path: hk_path.clone(),
            });

            // 4) flash by that path
            flash_halfkay_path(&hk_path, target_id, fw, opts, on_event)
        }
    }
}

fn flash_halfkay_path<F>(
    path: &str,
    target_id: &str,
    fw: &hex::FirmwareImage,
    opts: &FlashOptions,
    on_event: &mut F,
) -> Result<(), FlashError>
where
    F: FnMut(OperationEvent),
{
    let mut dev = halfkay::open_by_path(path).map_err(|e| FlashError::OpenHalfKay {
        path: path.to_string(),
        source: e,
    })?;

    on_event(OperationEvent::HalfKayOpen {
        target_id: target_id.to_string(),
        path: dev.path.clone(),
    });

    let total_to_write = fw.blocks_to_write.len();
    for (i, block_addr) in fw.blocks_to_write.iter().copied().enumerate() {
        on_event(OperationEvent::Block {
            target_id: target_id.to_string(),
            index: i,
            total: total_to_write,
            addr: block_addr,
        });

        let mut attempt: u32 = 0;
        loop {
            attempt = attempt.saturating_add(1);
            match halfkay::write_block_teensy41(&dev, fw, block_addr, i) {
                Ok(()) => break,
                Err(e) => {
                    if attempt > opts.retries {
                        return Err(FlashError::WriteFailed {
                            addr: block_addr,
                            attempts: attempt,
                            source: e,
                        });
                    }

                    on_event(OperationEvent::Retry {
                        target_id: target_id.to_string(),
                        addr: block_addr,
                        attempt,
                        retries: opts.retries,
                        error: e.to_string(),
                    });

                    std::thread::sleep(opts.reopen_delay);
                    dev = reopen_halfkay_by_path(path, opts.reopen_timeout).map_err(|e2| {
                        FlashError::ReopenFailed {
                            path: path.to_string(),
                            addr: block_addr,
                            source: e2,
                        }
                    })?;
                    std::thread::sleep(opts.reopen_delay);
                }
            }
        }
    }

    if !opts.no_reboot {
        on_event(OperationEvent::Boot {
            target_id: target_id.to_string(),
        });
        let _ = halfkay::boot_teensy41(&dev);
    }

    on_event(OperationEvent::Done {
        target_id: target_id.to_string(),
    });
    Ok(())
}

fn reopen_halfkay_by_path(
    path: &str,
    timeout: Duration,
) -> Result<halfkay::HalfKayDevice, halfkay::HalfKayError> {
    let start = Instant::now();
    loop {
        match halfkay::open_by_path(path) {
            Ok(d) => return Ok(d),
            Err(e) => {
                if start.elapsed() >= timeout {
                    return Err(e);
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::targets::{HalfKayTarget, SerialTarget};

    fn serial(port: &str) -> Target {
        Target::Serial(SerialTarget {
            port_name: port.to_string(),
            vid: 0x16C0,
            pid: 0x0483,
            serial_number: None,
            manufacturer: None,
            product: None,
        })
    }

    fn halfkay(path: &str) -> Target {
        Target::HalfKay(HalfKayTarget {
            vid: 0x16C0,
            pid: 0x0478,
            path: path.to_string(),
        })
    }

    #[test]
    fn select_targets_auto_prefers_single_halfkay() {
        let targets = vec![serial("COM5"), halfkay("HK1"), serial("COM6")];
        let mut events: Vec<OperationEvent> = Vec::new();

        let selected = select_targets(FlashSelection::Auto, None, &targets, true, &mut |e| {
            events.push(e)
        })
        .unwrap();

        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].id(), "halfkay:HK1");
        assert!(events.iter().any(
            |e| matches!(e, OperationEvent::TargetSelected { target_id } if target_id == "halfkay:HK1")
        ));
    }

    #[test]
    fn select_targets_auto_prefers_named_serial_port() {
        let targets = vec![serial("COM5"), serial("COM6")];
        let mut events: Vec<OperationEvent> = Vec::new();

        let selected = select_targets(
            FlashSelection::Auto,
            Some("COM6"),
            &targets,
            true,
            &mut |e| events.push(e),
        )
        .unwrap();

        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].id(), "serial:COM6");
        assert!(events.iter().any(
            |e| matches!(e, OperationEvent::TargetSelected { target_id } if target_id == "serial:COM6")
        ));
    }

    #[test]
    fn select_targets_does_not_emit_selected_when_disabled() {
        let targets = vec![serial("COM6")];
        let mut events: Vec<OperationEvent> = Vec::new();

        let selected = select_targets(FlashSelection::Auto, None, &targets, false, &mut |e| {
            events.push(e)
        })
        .unwrap();

        assert_eq!(selected.len(), 1);
        assert!(!events
            .iter()
            .any(|e| matches!(e, OperationEvent::TargetSelected { .. })));
    }
}
