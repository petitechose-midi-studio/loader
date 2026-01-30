use crate::bridge_control;
use crate::operation::OperationEvent;
use crate::targets::{Target, TargetKind};

pub(crate) fn run_targets_with_bridge<
    F,
    E,
    RunTarget,
    IsAmbiguous,
    MakeAmbiguous,
    MakeMultiFailed,
>(
    selected: Vec<Target>,
    bridge: &bridge_control::BridgeControlOptions,
    mut run_target: RunTarget,
    is_ambiguous: IsAmbiguous,
    make_ambiguous: MakeAmbiguous,
    make_multi_failed: MakeMultiFailed,
    on_event: &mut F,
) -> Result<(), E>
where
    F: FnMut(OperationEvent),
    E: std::fmt::Display,
    RunTarget: FnMut(&Target, &str, &mut F) -> Result<(), E>,
    IsAmbiguous: Fn(&E) -> bool,
    MakeAmbiguous: Fn(String) -> E,
    MakeMultiFailed: Fn(usize, usize) -> E,
{
    let total = selected.len();
    let multi = total > 1;

    let needs_serial = selected.iter().any(|t| t.kind() == TargetKind::Serial);
    let mut bridge_guard: Option<bridge_control::BridgeGuard> = None;
    if needs_serial {
        on_event(OperationEvent::BridgePauseStart);
        let paused = bridge_control::pause_oc_bridge(bridge);
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

    let mut failed = 0usize;
    let mut fatal_err: Option<E> = None;
    let mut ambiguous_message: Option<String> = None;

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
                if is_ambiguous(&e) && ambiguous_message.is_none() {
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

    let result = if let Some(e) = fatal_err {
        Err(e)
    } else if let Some(message) = ambiguous_message {
        Err(make_ambiguous(message))
    } else if failed > 0 {
        Err(make_multi_failed(failed, total))
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
