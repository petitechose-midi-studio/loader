use serde::Serialize;
use thiserror::Error;

use crate::{halfkay, teensy41};

pub const PJRC_VID: u16 = teensy41::VID;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum TargetKind {
    #[serde(rename = "halfkay")]
    HalfKay,
    Serial,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Target {
    #[serde(rename = "halfkay")]
    HalfKay(HalfKayTarget),
    Serial(SerialTarget),
}

impl Target {
    pub fn kind(&self) -> TargetKind {
        match self {
            Target::HalfKay(_) => TargetKind::HalfKay,
            Target::Serial(_) => TargetKind::Serial,
        }
    }

    pub fn id(&self) -> String {
        match self {
            Target::HalfKay(t) => format!("halfkay:{}", t.path),
            Target::Serial(t) => format!("serial:{}", t.port_name),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HalfKayTarget {
    pub vid: u16,
    pub pid: u16,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SerialTarget {
    pub port_name: String,
    pub vid: u16,
    pub pid: u16,
    pub serial_number: Option<String>,
    pub manufacturer: Option<String>,
    pub product: Option<String>,
}

#[derive(Error, Debug)]
pub enum DiscoverError {
    #[error("hid discovery failed: {0}")]
    Hid(#[from] halfkay::HalfKayError),

    #[error("serial discovery failed: {0}")]
    Serial(#[from] serialport::Error),
}

pub fn discover_targets() -> Result<Vec<Target>, DiscoverError> {
    let mut out: Vec<Target> = Vec::new();

    for d in halfkay::list_devices()? {
        out.push(Target::HalfKay(HalfKayTarget {
            vid: d.vid,
            pid: d.pid,
            path: d.path,
        }));
    }

    for p in serialport::available_ports()? {
        let serialport::SerialPortInfo {
            port_name,
            port_type,
        } = p;

        let serialport::SerialPortType::UsbPort(usb) = port_type else {
            continue;
        };

        if usb.vid != PJRC_VID {
            continue;
        }

        out.push(Target::Serial(SerialTarget {
            port_name,
            vid: usb.vid,
            pid: usb.pid,
            serial_number: usb.serial_number,
            manufacturer: usb.manufacturer,
            product: usb.product,
        }));
    }

    out.sort_by(|a, b| {
        let ak = a.kind();
        let bk = b.kind();
        if ak != bk {
            return ak.cmp(&bk);
        }
        match (a, b) {
            (Target::HalfKay(aa), Target::HalfKay(bb)) => aa.path.cmp(&bb.path),
            (Target::Serial(aa), Target::Serial(bb)) => aa.port_name.cmp(&bb.port_name),
            _ => std::cmp::Ordering::Equal,
        }
    });

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_target_id_format() {
        let t1 = Target::Serial(SerialTarget {
            port_name: "COM6".to_string(),
            vid: PJRC_VID,
            pid: 0x0489,
            serial_number: None,
            manufacturer: None,
            product: None,
        });
        assert_eq!(t1.id(), "serial:COM6");

        let t2 = Target::HalfKay(HalfKayTarget {
            vid: PJRC_VID,
            pid: teensy41::PID_HALFKAY,
            path: "\\\\?\\HID#VID_16C0&PID_0478#...".to_string(),
        });
        assert!(t2.id().starts_with("halfkay:"));
    }
}
