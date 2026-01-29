use std::time::Duration;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum SerialRebootError {
    #[error("no Teensy USB serial ports found")]
    NoTeensySerial,

    #[error("serial port error: {0}")]
    Serial(String),
}

pub fn soft_reboot_teensy41(preferred_port: Option<&str>) -> Result<String, SerialRebootError> {
    let ports =
        serialport::available_ports().map_err(|e| SerialRebootError::Serial(e.to_string()))?;
    let mut candidates: Vec<String> = Vec::new();

    for p in ports {
        if let serialport::SerialPortType::UsbPort(usb) = p.port_type {
            // PJRC VID for Teensy
            if usb.vid == 0x16C0 {
                candidates.push(p.port_name);
            }
        }
    }

    let port_name = if let Some(p) = preferred_port {
        p.to_string()
    } else {
        candidates
            .first()
            .cloned()
            .ok_or(SerialRebootError::NoTeensySerial)?
    };

    // The Teensyduino "134 baud" mechanism: setting line coding to 134 triggers reboot.
    // We only need to open the port and apply settings.
    let builder = serialport::new(&port_name, 134)
        .timeout(Duration::from_millis(500))
        .data_bits(serialport::DataBits::Eight)
        .parity(serialport::Parity::None)
        .stop_bits(serialport::StopBits::One)
        .flow_control(serialport::FlowControl::None);

    let mut port = builder
        .open()
        .map_err(|e| SerialRebootError::Serial(format!("{port_name}: {e}")))?;

    // Some drivers only send line coding on explicit set.
    let _ = port.set_baud_rate(134);
    std::thread::sleep(Duration::from_millis(120));
    drop(port);

    Ok(port_name)
}
