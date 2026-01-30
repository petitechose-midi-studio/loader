use midi_studio_loader::operation::OperationEvent;
use midi_studio_loader::targets::{self, HalfKayTarget, SerialTarget};

use super::human::HumanOutput;

#[test]
fn json_event_has_schema_and_event() {
    let ev = super::json::operation_event_to_json(OperationEvent::HexLoaded {
        bytes: 12,
        blocks: 3,
    });
    let v = serde_json::to_value(&ev).unwrap();
    assert_eq!(v.get("schema").and_then(|v| v.as_u64()), Some(1));
    assert_eq!(v.get("event").and_then(|v| v.as_str()), Some("hex_loaded"));
    assert_eq!(v.get("bytes").and_then(|v| v.as_u64()), Some(12));
    assert_eq!(v.get("blocks").and_then(|v| v.as_u64()), Some(3));
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
