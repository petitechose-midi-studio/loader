use std::collections::HashSet;
use std::time::Duration;

use thiserror::Error;

use crate::api::{FlashEvent, FlashSelection};
use crate::{
    bootloader, bridge_control, halfkay, serial_reboot, targets,
    targets::{Target, TargetKind},
};

#[derive(Debug, Clone)]
pub struct RebootOptions {
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
    F: FnMut(FlashEvent),
{
    on_event(FlashEvent::DiscoverStart);
    let targets =
        targets::discover_targets().map_err(|e| RebootError::DiscoveryFailed { source: e })?;

    for (index, target) in targets.iter().cloned().enumerate() {
        on_event(FlashEvent::TargetDetected { index, target });
    }

    on_event(FlashEvent::DiscoverDone {
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

    let needs_serial = selected.iter().any(|t| t.kind() == TargetKind::Serial);
    let mut bridge_guard: Option<bridge_control::BridgeGuard> = None;
    if needs_serial {
        on_event(FlashEvent::BridgePauseStart);
        let paused = bridge_control::pause_oc_bridge(&opts.bridge);
        match &paused.outcome {
            bridge_control::BridgePauseOutcome::Paused(info) => {
                on_event(FlashEvent::BridgePaused { info: info.clone() });
            }
            bridge_control::BridgePauseOutcome::Skipped(reason) => {
                on_event(FlashEvent::BridgePauseSkipped {
                    reason: reason.clone(),
                });
            }
            bridge_control::BridgePauseOutcome::Failed(error) => {
                on_event(FlashEvent::BridgePauseFailed {
                    error: error.clone(),
                });
            }
        }
        bridge_guard = paused.guard;
    }

    let total = selected.len();
    let multi = total > 1;
    let mut failed = 0usize;
    let mut fatal_err: Option<RebootError> = None;
    let mut ambiguous_message: Option<String> = None;

    for target in selected {
        let target_id = target.id();
        on_event(FlashEvent::TargetStart {
            target_id: target_id.clone(),
            kind: target.kind(),
        });

        let r = reboot_one_target(&target, &target_id, opts, &mut on_event);
        match r {
            Ok(()) => {
                on_event(FlashEvent::TargetDone {
                    target_id,
                    ok: true,
                    message: None,
                });
            }
            Err(e) => {
                failed += 1;
                if let RebootError::AmbiguousTarget { message } = &e {
                    if ambiguous_message.is_none() {
                        ambiguous_message = Some(message.clone());
                    }
                }
                on_event(FlashEvent::TargetDone {
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
    } else if let Some(message) = ambiguous_message {
        Err(RebootError::AmbiguousTarget { message })
    } else if failed > 0 {
        Err(RebootError::MultiTargetFailed { failed, total })
    } else {
        Ok(())
    };

    if let Some(mut g) = bridge_guard {
        on_event(FlashEvent::BridgeResumeStart);
        let hint = g.resume_hint();
        match g.resume() {
            Ok(()) => on_event(FlashEvent::BridgeResumed),
            Err(e) => on_event(FlashEvent::BridgeResumeFailed {
                error: bridge_control::BridgeControlErrorInfo {
                    message: format!("bridge resume failed: {e}"),
                    hint,
                },
            }),
        }
    }

    result
}

fn reboot_one_target<F>(
    target: &Target,
    target_id: &str,
    opts: &RebootOptions,
    on_event: &mut F,
) -> Result<(), RebootError>
where
    F: FnMut(FlashEvent),
{
    match target {
        Target::HalfKay(t) => {
            on_event(FlashEvent::HalfKayOpen {
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
                    on_event(FlashEvent::SoftReboot {
                        target_id: target_id.to_string(),
                        port: t.port_name.clone(),
                    });
                    std::thread::sleep(opts.soft_reboot_delay);
                }
                Err(e) => {
                    on_event(FlashEvent::SoftRebootSkipped {
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

            on_event(FlashEvent::HalfKayAppeared {
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
