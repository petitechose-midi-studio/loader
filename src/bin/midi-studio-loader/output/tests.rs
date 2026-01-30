use std::collections::BTreeSet;

use midi_studio_loader::bridge_control::{
    BridgeControlErrorInfo, BridgePauseInfo, BridgePauseMethod, BridgePauseSkipReason,
    OcBridgeProcessInfo, ServiceStatus,
};
use midi_studio_loader::operation::OperationEvent;
use midi_studio_loader::targets::{self, HalfKayTarget, SerialTarget, TargetKind};

use super::human::HumanOutput;

fn keys(v: &serde_json::Value) -> BTreeSet<String> {
    v.as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>()
}

fn assert_json_event<F>(ev: OperationEvent, expected_event: &str, expected_keys: &[&str], check: F)
where
    F: FnOnce(&serde_json::Value),
{
    let je = super::json::operation_event_to_json(ev);
    let v = serde_json::to_value(&je).unwrap();

    assert_eq!(v.get("schema").and_then(|v| v.as_u64()), Some(1));
    assert_eq!(
        v.get("event").and_then(|v| v.as_str()),
        Some(expected_event)
    );

    let expected: BTreeSet<String> = expected_keys.iter().map(|s| s.to_string()).collect();
    assert_eq!(keys(&v), expected);

    check(&v);
}

#[test]
fn json_event_has_schema_and_event() {
    assert_json_event(
        OperationEvent::HexLoaded {
            bytes: 12,
            blocks: 3,
        },
        "hex_loaded",
        &["schema", "event", "bytes", "blocks"],
        |v| {
            assert_eq!(v.get("bytes").and_then(|v| v.as_u64()), Some(12));
            assert_eq!(v.get("blocks").and_then(|v| v.as_u64()), Some(3));
        },
    );
}

#[test]
fn operation_event_json_contract() {
    assert_json_event(
        OperationEvent::DiscoverStart,
        "discover_start",
        &["schema", "event"],
        |_| {},
    );

    assert_json_event(
        OperationEvent::TargetDetected {
            index: 2,
            target: targets::Target::Serial(SerialTarget {
                port_name: "COM6".to_string(),
                vid: 0x16C0,
                pid: 0x0483,
                serial_number: None,
                manufacturer: None,
                product: None,
            }),
        },
        "target_detected",
        &["schema", "event", "index", "target_id", "kind"],
        |v| {
            assert_eq!(v.get("index").and_then(|v| v.as_u64()), Some(2));
            assert_eq!(
                v.get("target_id").and_then(|v| v.as_str()),
                Some("serial:COM6")
            );
            assert_eq!(v.get("kind").and_then(|v| v.as_str()), Some("serial"));
        },
    );

    assert_json_event(
        OperationEvent::DiscoverDone { count: 3 },
        "discover_done",
        &["schema", "event", "count"],
        |v| {
            assert_eq!(v.get("count").and_then(|v| v.as_u64()), Some(3));
        },
    );

    assert_json_event(
        OperationEvent::TargetSelected {
            target_id: "halfkay:abc".to_string(),
        },
        "target_selected",
        &["schema", "event", "target_id"],
        |v| {
            assert_eq!(
                v.get("target_id").and_then(|v| v.as_str()),
                Some("halfkay:abc")
            );
        },
    );

    assert_json_event(
        OperationEvent::BridgePauseStart,
        "bridge_pause_start",
        &["schema", "event"],
        |_| {},
    );

    assert_json_event(
        OperationEvent::BridgePaused {
            info: BridgePauseInfo {
                method: BridgePauseMethod::Control,
                id: "127.0.0.1:7999".to_string(),
                pids: vec![1234, 5678],
            },
        },
        "bridge_paused",
        &["schema", "event", "method", "id", "pids"],
        |v| {
            assert_eq!(v.get("method").and_then(|v| v.as_str()), Some("control"));
            assert_eq!(v.get("id").and_then(|v| v.as_str()), Some("127.0.0.1:7999"));
            assert_eq!(
                v.get("pids").and_then(|v| v.as_array()).map(|a| a.len()),
                Some(2)
            );
        },
    );

    assert_json_event(
        OperationEvent::BridgePauseSkipped {
            reason: BridgePauseSkipReason::Disabled,
        },
        "bridge_pause_skipped",
        &["schema", "event", "reason"],
        |v| {
            assert_eq!(v.get("reason").and_then(|v| v.as_str()), Some("disabled"));
        },
    );

    assert_json_event(
        OperationEvent::BridgePauseFailed {
            error: BridgeControlErrorInfo {
                message: "nope".to_string(),
                hint: None,
            },
        },
        "bridge_pause_failed",
        &["schema", "event", "message"],
        |v| {
            assert_eq!(v.get("message").and_then(|v| v.as_str()), Some("nope"));
        },
    );

    assert_json_event(
        OperationEvent::BridgePauseFailed {
            error: BridgeControlErrorInfo {
                message: "nope".to_string(),
                hint: Some("try X".to_string()),
            },
        },
        "bridge_pause_failed",
        &["schema", "event", "message", "hint"],
        |v| {
            assert_eq!(v.get("hint").and_then(|v| v.as_str()), Some("try X"));
        },
    );

    assert_json_event(
        OperationEvent::BridgeResumeStart,
        "bridge_resume_start",
        &["schema", "event"],
        |_| {},
    );
    assert_json_event(
        OperationEvent::BridgeResumed,
        "bridge_resumed",
        &["schema", "event"],
        |_| {},
    );

    assert_json_event(
        OperationEvent::BridgeResumeFailed {
            error: BridgeControlErrorInfo {
                message: "resume failed".to_string(),
                hint: Some("try Y".to_string()),
            },
        },
        "bridge_resume_failed",
        &["schema", "event", "message", "hint"],
        |_| {},
    );

    assert_json_event(
        OperationEvent::TargetStart {
            target_id: "serial:COM6".to_string(),
            kind: TargetKind::Serial,
        },
        "target_start",
        &["schema", "event", "target_id", "kind"],
        |v| {
            assert_eq!(v.get("kind").and_then(|v| v.as_str()), Some("serial"));
        },
    );

    assert_json_event(
        OperationEvent::TargetDone {
            target_id: "serial:COM6".to_string(),
            ok: true,
            message: None,
        },
        "target_done",
        &["schema", "event", "target_id", "ok"],
        |v| {
            assert_eq!(v.get("ok").and_then(|v| v.as_u64()), Some(1));
        },
    );

    assert_json_event(
        OperationEvent::TargetDone {
            target_id: "serial:COM6".to_string(),
            ok: false,
            message: Some("boom".to_string()),
        },
        "target_done",
        &["schema", "event", "target_id", "ok", "message"],
        |v| {
            assert_eq!(v.get("ok").and_then(|v| v.as_u64()), Some(0));
            assert_eq!(v.get("message").and_then(|v| v.as_str()), Some("boom"));
        },
    );

    assert_json_event(
        OperationEvent::SoftReboot {
            target_id: "serial:COM6".to_string(),
            port: "COM6".to_string(),
        },
        "soft_reboot",
        &["schema", "event", "target_id", "port"],
        |_| {},
    );

    assert_json_event(
        OperationEvent::SoftRebootSkipped {
            target_id: "serial:COM6".to_string(),
            error: "no serial".to_string(),
        },
        "soft_reboot_skipped",
        &["schema", "event", "target_id", "message"],
        |v| {
            assert_eq!(v.get("message").and_then(|v| v.as_str()), Some("no serial"));
        },
    );

    assert_json_event(
        OperationEvent::HalfKayAppeared {
            target_id: "serial:COM6".to_string(),
            path: "HK1".to_string(),
        },
        "halfkay_appeared",
        &["schema", "event", "target_id", "path"],
        |_| {},
    );
    assert_json_event(
        OperationEvent::HalfKayOpen {
            target_id: "halfkay:HK1".to_string(),
            path: "HK1".to_string(),
        },
        "halfkay_open",
        &["schema", "event", "target_id", "path"],
        |_| {},
    );

    assert_json_event(
        OperationEvent::Block {
            target_id: "halfkay:HK1".to_string(),
            index: 5,
            total: 10,
            addr: 0x400,
        },
        "block",
        &["schema", "event", "target_id", "i", "n", "addr"],
        |v| {
            assert_eq!(v.get("i").and_then(|v| v.as_u64()), Some(5));
            assert_eq!(v.get("n").and_then(|v| v.as_u64()), Some(10));
            assert_eq!(v.get("addr").and_then(|v| v.as_u64()), Some(0x400));
        },
    );

    assert_json_event(
        OperationEvent::Retry {
            target_id: "halfkay:HK1".to_string(),
            addr: 0x400,
            attempt: 2,
            retries: 3,
            error: "short write".to_string(),
        },
        "retry",
        &[
            "schema",
            "event",
            "target_id",
            "addr",
            "attempt",
            "retries",
            "error",
        ],
        |v| {
            assert_eq!(v.get("attempt").and_then(|v| v.as_u64()), Some(2));
        },
    );

    assert_json_event(
        OperationEvent::Boot {
            target_id: "halfkay:HK1".to_string(),
        },
        "boot",
        &["schema", "event", "target_id"],
        |_| {},
    );
    assert_json_event(
        OperationEvent::Done {
            target_id: "halfkay:HK1".to_string(),
        },
        "done",
        &["schema", "event", "target_id"],
        |_| {},
    );
}

#[test]
fn ambiguous_help_includes_targets() {
    let detected = vec![
        Some(targets::Target::Serial(SerialTarget {
            port_name: "COM6".to_string(),
            vid: 0x16C0,
            pid: 0x0483,
            serial_number: None,
            manufacturer: None,
            product: Some("MIDI Studio".to_string()),
        })),
        Some(targets::Target::HalfKay(HalfKayTarget {
            vid: 0x16C0,
            pid: 0x0478,
            path: "abc".to_string(),
        })),
    ];

    let lines = HumanOutput::ambiguous_help_lines(&detected);
    assert!(lines.iter().any(|l| l.contains("serial:COM6")));
    assert!(lines.iter().any(|l| l.contains("halfkay:abc")));
}

#[test]
fn dry_run_json_contract() {
    let ev = super::json::dry_run_to_json(super::DryRunSummary {
        bytes: 123,
        blocks: 10,
        blocks_to_write: 2,
        target_ids: vec!["serial:COM6".to_string()],
        needs_serial: true,
        bridge_enabled: true,
        bridge_control_port: 7999,
    });
    let v = serde_json::to_value(&ev).unwrap();
    assert_eq!(v.get("event").and_then(|v| v.as_str()), Some("dry_run"));
    assert_eq!(v.get("targets").and_then(|v| v.as_u64()), Some(1));
    assert_eq!(
        v.get("target_ids")
            .and_then(|v| v.as_array())
            .map(|a| a.len()),
        Some(1)
    );
}

#[test]
fn doctor_json_contract_minimal() {
    let report = super::DoctorReport {
        service_id: "OpenControlBridge".to_string(),
        targets: vec![targets::Target::HalfKay(HalfKayTarget {
            vid: 0x16C0,
            pid: 0x0478,
            path: "HK".to_string(),
        })],
        control_port: 7999,
        control_timeout_ms: 2500,
        control_checked: false,
        control: None,
        control_error: None,
        service_status: Some(ServiceStatus::Stopped),
        service_error: None,
        processes: vec![OcBridgeProcessInfo {
            pid: 1234,
            exe: None,
            cmd: None,
            restartable: false,
        }],
    };

    let ev = super::json::doctor_to_json(report);
    let v = serde_json::to_value(&ev).unwrap();
    assert_eq!(v.get("event").and_then(|v| v.as_str()), Some("doctor"));
    assert_eq!(
        v.get("service_id").and_then(|v| v.as_str()),
        Some("OpenControlBridge")
    );
    assert_eq!(v.get("control_checked").and_then(|v| v.as_u64()), Some(0));
    assert_eq!(
        v.get("targets").and_then(|v| v.as_array()).map(|a| a.len()),
        Some(1)
    );
}
