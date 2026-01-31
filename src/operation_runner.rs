use crate::bridge_control;
use crate::operation::OperationEvent;
use crate::targets::{Target, TargetKind};

pub(crate) struct RunTargetsErrors<
    IsAmbiguous,
    MakeAmbiguous,
    MakeMultiFailed,
    MakeBridgePauseFailed,
> {
    pub is_ambiguous: IsAmbiguous,
    pub make_ambiguous: MakeAmbiguous,
    pub make_multi_failed: MakeMultiFailed,
    pub make_bridge_pause_failed: MakeBridgePauseFailed,
}

pub(crate) fn run_targets_with_bridge<
    F,
    E,
    RunTarget,
    IsAmbiguous,
    MakeAmbiguous,
    MakeMultiFailed,
    MakeBridgePauseFailed,
    PauseBridge,
>(
    selected: Vec<Target>,
    bridge: &bridge_control::BridgeControlOptions,
    pause_bridge: PauseBridge,
    mut run_target: RunTarget,
    errors: RunTargetsErrors<IsAmbiguous, MakeAmbiguous, MakeMultiFailed, MakeBridgePauseFailed>,
    on_event: &mut F,
) -> Result<(), E>
where
    F: FnMut(OperationEvent),
    E: std::fmt::Display,
    PauseBridge: FnOnce(&bridge_control::BridgeControlOptions) -> bridge_control::BridgePause,
    RunTarget: FnMut(&Target, &str, &mut F) -> Result<(), E>,
    IsAmbiguous: Fn(&E) -> bool,
    MakeAmbiguous: Fn(String) -> E,
    MakeMultiFailed: Fn(usize, usize) -> E,
    MakeBridgePauseFailed: Fn(bridge_control::BridgeControlErrorInfo) -> E,
{
    let total = selected.len();
    let multi = total > 1;

    let needs_serial = selected.iter().any(|t| t.kind() == TargetKind::Serial);

    let mut failed = 0usize;
    let mut fatal_err: Option<E> = None;
    let mut ambiguous_message: Option<String> = None;

    let mut bridge_guard: Option<bridge_control::BridgeGuard> = None;
    if needs_serial {
        on_event(OperationEvent::BridgePauseStart);
        let paused = pause_bridge(bridge);
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
                // Safety-first: if we needed serial but couldn't pause the bridge,
                // abort before attempting any device operations.
                return Err((errors.make_bridge_pause_failed)(error.clone()));
            }
        }
        bridge_guard = paused.guard;
    }

    if fatal_err.is_none() {
        for target in selected {
            let target_id = target.id();
            on_event(OperationEvent::TargetStart {
                target_id: target_id.clone(),
                kind: target.kind(),
            });

            match run_target(&target, &target_id, on_event) {
                Ok(()) => {
                    on_event(OperationEvent::TargetDone {
                        target_id,
                        ok: true,
                        message: None,
                    });
                }
                Err(e) => {
                    failed += 1;
                    if (errors.is_ambiguous)(&e) && ambiguous_message.is_none() {
                        ambiguous_message = Some(e.to_string());
                    }

                    on_event(OperationEvent::TargetDone {
                        target_id,
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
    }

    let result = if let Some(e) = fatal_err {
        Err(e)
    } else if let Some(message) = ambiguous_message {
        Err((errors.make_ambiguous)(message))
    } else if failed > 0 {
        Err((errors.make_multi_failed)(failed, total))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::targets::{HalfKayTarget, SerialTarget, Target, PJRC_VID};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    #[derive(Debug)]
    struct DummyError(String);

    impl std::fmt::Display for DummyError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{}", self.0)
        }
    }

    fn serial_target(port: &str) -> Target {
        Target::Serial(SerialTarget {
            port_name: port.to_string(),
            vid: PJRC_VID,
            pid: 0x0489,
            serial_number: None,
            manufacturer: None,
            product: None,
        })
    }

    fn halfkay_target(path: &str) -> Target {
        Target::HalfKay(HalfKayTarget {
            vid: PJRC_VID,
            pid: crate::teensy41::PID_HALFKAY,
            path: path.to_string(),
        })
    }

    #[test]
    fn pause_failed_aborts_before_touching_device() {
        let selected = vec![serial_target("COM6")];
        let opts = bridge_control::BridgeControlOptions {
            enabled: true,
            method: bridge_control::BridgeControlMethod::Control,
            control_port: 7999,
            control_timeout: Duration::from_millis(1),
            timeout: Duration::from_millis(1),
            service_id: None,
            allow_process_fallback: false,
        };

        let ran = Arc::new(Mutex::new(false));
        let ran2 = ran.clone();
        let events: Arc<Mutex<Vec<OperationEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let events2 = events.clone();

        let res = run_targets_with_bridge(
            selected,
            &opts,
            |_opts| bridge_control::BridgePause {
                guard: None,
                outcome: bridge_control::BridgePauseOutcome::Failed(
                    bridge_control::BridgeControlErrorInfo {
                        message: "pause failed".to_string(),
                        hint: None,
                    },
                ),
            },
            |_target, _target_id, _on_event| {
                *ran2.lock().unwrap() = true;
                Ok(())
            },
            RunTargetsErrors {
                is_ambiguous: |_e: &DummyError| false,
                make_ambiguous: DummyError,
                make_multi_failed: |_failed, _total| DummyError("multi".to_string()),
                make_bridge_pause_failed: |err: bridge_control::BridgeControlErrorInfo| {
                    DummyError(err.message)
                },
            },
            &mut |ev| events2.lock().unwrap().push(ev),
        );

        assert!(res.is_err());
        assert!(!*ran.lock().unwrap());

        let evs = events.lock().unwrap();
        assert!(evs
            .iter()
            .any(|e| matches!(e, OperationEvent::BridgePauseStart)));
        assert!(evs
            .iter()
            .any(|e| matches!(e, OperationEvent::BridgePauseFailed { .. })));
        assert!(!evs
            .iter()
            .any(|e| matches!(e, OperationEvent::SoftReboot { .. })));
        assert!(!evs
            .iter()
            .any(|e| matches!(e, OperationEvent::Block { .. })));
    }

    #[test]
    fn resume_events_emitted_even_when_target_fails() {
        let selected = vec![serial_target("COM6")];
        let opts = bridge_control::BridgeControlOptions {
            enabled: true,
            method: bridge_control::BridgeControlMethod::Control,
            control_port: 7999,
            control_timeout: Duration::from_millis(1),
            timeout: Duration::from_millis(1),
            service_id: None,
            allow_process_fallback: false,
        };

        let events: Arc<Mutex<Vec<OperationEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let events2 = events.clone();

        let res = run_targets_with_bridge(
            selected,
            &opts,
            |_opts| bridge_control::BridgePause {
                guard: Some(bridge_control::test_noop_guard()),
                outcome: bridge_control::BridgePauseOutcome::Paused(
                    bridge_control::BridgePauseInfo {
                        method: bridge_control::BridgePauseMethod::Control,
                        id: "127.0.0.1:7999".to_string(),
                        pids: Vec::new(),
                    },
                ),
            },
            |_target, _target_id, _on_event| Err(DummyError("boom".to_string())),
            RunTargetsErrors {
                is_ambiguous: |_e: &DummyError| false,
                make_ambiguous: DummyError,
                make_multi_failed: |_failed, _total| DummyError("multi".to_string()),
                make_bridge_pause_failed: |err: bridge_control::BridgeControlErrorInfo| {
                    DummyError(err.message)
                },
            },
            &mut |ev| events2.lock().unwrap().push(ev),
        );

        assert!(res.is_err());

        let evs = events.lock().unwrap();
        let has_resume_start = evs
            .iter()
            .any(|e| matches!(e, OperationEvent::BridgeResumeStart));
        let has_resumed = evs
            .iter()
            .any(|e| matches!(e, OperationEvent::BridgeResumed));
        assert!(has_resume_start);
        assert!(has_resumed);
    }

    #[test]
    fn halfkay_targets_do_not_pause_bridge() {
        let selected = vec![halfkay_target("\\\\?\\HID#VID_16C0&PID_0478#TEST")];
        let opts = bridge_control::BridgeControlOptions {
            enabled: true,
            method: bridge_control::BridgeControlMethod::Auto,
            control_port: 7999,
            control_timeout: Duration::from_millis(1),
            timeout: Duration::from_millis(1),
            service_id: None,
            allow_process_fallback: false,
        };

        let events: Arc<Mutex<Vec<OperationEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let events2 = events.clone();

        let res = run_targets_with_bridge(
            selected,
            &opts,
            |_opts| panic!("pause bridge should not be called for halfkay targets"),
            |_target, _target_id, _on_event| Ok(()),
            RunTargetsErrors {
                is_ambiguous: |_e: &DummyError| false,
                make_ambiguous: DummyError,
                make_multi_failed: |_failed, _total| DummyError("multi".to_string()),
                make_bridge_pause_failed: |err: bridge_control::BridgeControlErrorInfo| {
                    DummyError(err.message)
                },
            },
            &mut |ev| events2.lock().unwrap().push(ev),
        );

        assert!(res.is_ok());
        let evs = events.lock().unwrap();
        assert!(!evs
            .iter()
            .any(|e| matches!(e, OperationEvent::BridgePauseStart)));
        assert!(!evs
            .iter()
            .any(|e| matches!(e, OperationEvent::BridgePaused { .. })));
        assert!(!evs
            .iter()
            .any(|e| matches!(e, OperationEvent::BridgePauseFailed { .. })));
        assert!(!evs
            .iter()
            .any(|e| matches!(e, OperationEvent::BridgeResumeStart)));
        assert!(!evs
            .iter()
            .any(|e| matches!(e, OperationEvent::BridgeResumed)));
    }
}
