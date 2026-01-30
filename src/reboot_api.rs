use std::collections::HashSet;
use std::time::Duration;

use thiserror::Error;
use tracing::{debug, warn};

use crate::api::FlashSelection;
use crate::operation::OperationEvent;
use crate::{bootloader, bridge_control, halfkay, serial_reboot, targets, targets::Target};

#[derive(Debug, Clone)]
pub struct RebootOptions {
    /// Reboot behavior and bridge coordination options.
    ///
    /// Defaults aim to be safe and reliable.
    /// Prefer a specific serial port name when selecting among multiple Serial targets.
    ///
    /// Example: "COM6" or "/dev/ttyACM0".
    pub serial_port: Option<String>,

    /// Max time to wait for HalfKay to appear after a serial soft reboot.
    ///
    /// None = wait forever.
    pub wait_timeout: Option<Duration>,

    /// Poll interval while waiting for HalfKay.
    pub poll_interval: Duration,

    /// Delay after triggering a serial reboot before polling for HalfKay.
    pub soft_reboot_delay: Duration,

    pub bridge: bridge_control::BridgeControlOptions,
}

impl Default for RebootOptions {
    fn default() -> Self {
        Self {
            serial_port: None,
            wait_timeout: Some(Duration::from_secs(60)),
            poll_interval: Duration::from_millis(50),
            soft_reboot_delay: Duration::from_millis(250),
            bridge: bridge_control::BridgeControlOptions::default(),
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum RebootErrorKind {
    NoDevice,
    AmbiguousTarget,
    Unexpected,
}

#[derive(Error, Debug)]
pub enum RebootError {
    #[error("no target device found")]
    NoTargets,

    #[error("ambiguous target: {message}")]
    AmbiguousTarget { message: String },

    #[error("target discovery failed: {source}")]
    DiscoveryFailed {
        #[source]
        source: targets::DiscoverError,
    },

    #[error("soft reboot failed on {port}: {source}")]
    SoftRebootFailed {
        port: String,
        #[source]
        source: serial_reboot::SerialRebootError,
    },

    #[error("HalfKay did not appear after soft reboot")]
    HalfKayTimeout,

    #[error("unable to pause oc-bridge: {message}")]
    BridgePauseFailed { message: String },

    #[error("reboot failed for {failed}/{total} targets")]
    MultiTargetFailed { failed: usize, total: usize },

    #[error("{message}")]
    Unexpected { message: String },
}

impl RebootError {
    pub fn kind(&self) -> RebootErrorKind {
        match self {
            RebootError::NoTargets => RebootErrorKind::NoDevice,
            RebootError::AmbiguousTarget { .. } => RebootErrorKind::AmbiguousTarget,
            RebootError::DiscoveryFailed { .. } => RebootErrorKind::Unexpected,
            RebootError::SoftRebootFailed { .. } => RebootErrorKind::NoDevice,
            RebootError::HalfKayTimeout => RebootErrorKind::NoDevice,
            RebootError::BridgePauseFailed { .. } => RebootErrorKind::Unexpected,
            RebootError::MultiTargetFailed { .. } => RebootErrorKind::NoDevice,
            RebootError::Unexpected { .. } => RebootErrorKind::Unexpected,
        }
    }
}

pub fn reboot_teensy41_with_selection<F>(
    opts: &RebootOptions,
    selection: FlashSelection,
    mut on_event: F,
) -> Result<(), RebootError>
where
    F: FnMut(OperationEvent),
{
    debug!("reboot teensy41 with selection");
    on_event(OperationEvent::DiscoverStart);
    let targets =
        targets::discover_targets().map_err(|e| RebootError::DiscoveryFailed { source: e })?;

    for (index, target) in targets.iter().cloned().enumerate() {
        on_event(OperationEvent::TargetDetected { index, target });
    }

    on_event(OperationEvent::DiscoverDone {
        count: targets.len(),
    });

    if targets.is_empty() {
        return Err(RebootError::NoTargets);
    }

    let selected = crate::api::select_targets(
        selection,
        opts.serial_port.as_deref(),
        &targets,
        true,
        &mut on_event,
    )
    .map_err(|e| match e {
        crate::api::FlashError::NoTargets => RebootError::NoTargets,
        crate::api::FlashError::AmbiguousTarget { message } => {
            RebootError::AmbiguousTarget { message }
        }
        other => RebootError::Unexpected {
            message: other.to_string(),
        },
    })?;

    crate::operation_runner::run_targets_with_bridge(
        selected,
        &opts.bridge,
        bridge_control::pause_oc_bridge,
        |target, target_id, on_event| reboot_one_target(target, target_id, opts, on_event),
        |e| matches!(e.kind(), RebootErrorKind::AmbiguousTarget),
        |message| RebootError::AmbiguousTarget { message },
        |failed, total| RebootError::MultiTargetFailed { failed, total },
        |err| {
            let mut msg = err.message;
            if let Some(hint) = err.hint {
                msg = format!("{msg} ({hint})");
            }
            RebootError::BridgePauseFailed { message: msg }
        },
        &mut on_event,
    )
}

fn reboot_one_target<F>(
    target: &Target,
    target_id: &str,
    opts: &RebootOptions,
    on_event: &mut F,
) -> Result<(), RebootError>
where
    F: FnMut(OperationEvent),
{
    debug!(target_id = target_id, kind = ?target.kind(), "reboot target");
    match target {
        Target::HalfKay(t) => {
            on_event(OperationEvent::HalfKayOpen {
                target_id: target_id.to_string(),
                path: t.path.clone(),
            });
            Ok(())
        }

        Target::Serial(t) => {
            let before = halfkay::list_paths().map_err(|e| RebootError::DiscoveryFailed {
                source: targets::DiscoverError::Hid(e),
            })?;
            let before: HashSet<String> = before.into_iter().collect();

            match serial_reboot::soft_reboot_port(&t.port_name) {
                Ok(()) => {
                    on_event(OperationEvent::SoftReboot {
                        target_id: target_id.to_string(),
                        port: t.port_name.clone(),
                    });
                    std::thread::sleep(opts.soft_reboot_delay);
                }
                Err(e) => {
                    warn!(target_id = target_id, port = %t.port_name, err = %e, "soft reboot failed");
                    on_event(OperationEvent::SoftRebootSkipped {
                        target_id: target_id.to_string(),
                        error: e.to_string(),
                    });
                    return Err(RebootError::SoftRebootFailed {
                        port: t.port_name.clone(),
                        source: e,
                    });
                }
            }

            let path = wait_for_new_halfkay(&before, opts.wait_timeout, opts.poll_interval)?;

            on_event(OperationEvent::HalfKayAppeared {
                target_id: target_id.to_string(),
                path: path.clone(),
            });
            Ok(())
        }
    }
}

fn wait_for_new_halfkay(
    before: &HashSet<String>,
    timeout: Option<Duration>,
    poll_interval: Duration,
) -> Result<String, RebootError> {
    match timeout {
        Some(t) => {
            bootloader::wait_for_new_halfkay(before, t, poll_interval).map_err(map_wait_error)
        }
        None => loop {
            let now = halfkay::list_paths().map_err(|e| RebootError::DiscoveryFailed {
                source: targets::DiscoverError::Hid(e),
            })?;
            match bootloader::diff_new_halfkay(before, &now) {
                Ok(Some(p)) => return Ok(p),
                Ok(None) => {}
                Err(e) => return Err(map_wait_error(e)),
            }
            std::thread::sleep(poll_interval);
        },
    }
}

fn map_wait_error(e: bootloader::WaitHalfKayError) -> RebootError {
    match e {
        bootloader::WaitHalfKayError::Ambiguous { count } => RebootError::AmbiguousTarget {
            message: format!("multiple new HalfKay devices appeared ({count})"),
        },
        bootloader::WaitHalfKayError::Timeout => RebootError::HalfKayTimeout,
        bootloader::WaitHalfKayError::ListFailed(e) => RebootError::DiscoveryFailed {
            source: targets::DiscoverError::Hid(e),
        },
    }
}
