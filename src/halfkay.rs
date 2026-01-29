use std::time::{Duration, Instant};

use hidapi::{HidApi, HidDevice};
use thiserror::Error;

use crate::hex::FirmwareImage;
use crate::teensy41;

#[derive(serde::Serialize)]
pub struct HalfKayDeviceSummary {
    pub vid: u16,
    pub pid: u16,
    pub path: String,
}

pub struct HalfKayDevice {
    _api: HidApi,
    dev: HidDevice,
    pub path: String,
}

#[derive(Error, Debug)]
pub enum HalfKayError {
    #[error("hid: {0}")]
    Hid(#[from] hidapi::HidError),

    #[error("no HalfKay device found")]
    NoDevice,

    #[error("short write: {got} != {expected}")]
    ShortWrite { got: usize, expected: usize },
}

pub fn list_devices() -> Result<Vec<HalfKayDeviceSummary>, HalfKayError> {
    let api = HidApi::new()?;
    let mut out: Vec<HalfKayDeviceSummary> = Vec::new();
    for d in api.device_list() {
        if d.vendor_id() == teensy41::VID && d.product_id() == teensy41::PID_HALFKAY {
            out.push(HalfKayDeviceSummary {
                vid: d.vendor_id(),
                pid: d.product_id(),
                path: d.path().to_string_lossy().to_string(),
            });
        }
    }
    Ok(out)
}

pub fn open_halfkay_device(
    wait: bool,
    wait_timeout: Option<Duration>,
) -> Result<HalfKayDevice, HalfKayError> {
    let start = Instant::now();

    loop {
        let api = HidApi::new()?;

        // Find a path first (so we can display it), then open via VID/PID.
        let path = api
            .device_list()
            .find(|d| d.vendor_id() == teensy41::VID && d.product_id() == teensy41::PID_HALFKAY)
            .map(|d| d.path().to_string_lossy().to_string());

        if let Some(path) = path {
            let dev = api.open(teensy41::VID, teensy41::PID_HALFKAY)?;
            return Ok(HalfKayDevice {
                _api: api,
                dev,
                path,
            });
        }

        if !wait {
            return Err(HalfKayError::NoDevice);
        }
        if let Some(t) = wait_timeout {
            if start.elapsed() >= t {
                return Err(HalfKayError::NoDevice);
            }
        }

        std::thread::sleep(Duration::from_millis(250));
    }
}

pub fn write_block_teensy41(
    dev: &HalfKayDevice,
    fw: &FirmwareImage,
    block_addr: usize,
) -> Result<(), HalfKayError> {
    let end = block_addr + teensy41::BLOCK_SIZE;
    let report = build_block_report_teensy41(block_addr, &fw.data[block_addr..end]);
    let n = dev.dev.write(&report)?;
    if n != report.len() {
        return Err(HalfKayError::ShortWrite {
            got: n,
            expected: report.len(),
        });
    }
    Ok(())
}

pub fn boot_teensy41(dev: &HalfKayDevice) -> Result<(), HalfKayError> {
    let report = build_boot_report_teensy41();
    let _ = dev.dev.write(&report)?;
    Ok(())
}

pub fn build_block_report_teensy41(block_addr: usize, data: &[u8]) -> Vec<u8> {
    assert_eq!(data.len(), teensy41::BLOCK_SIZE);

    // First byte is Report ID (0).
    let mut report = vec![0u8; teensy41::PACKET_SIZE + 1];
    let pkt = &mut report[1..];

    let addr = block_addr as u32;
    pkt[0] = (addr & 0xFF) as u8;
    pkt[1] = ((addr >> 8) & 0xFF) as u8;
    pkt[2] = ((addr >> 16) & 0xFF) as u8;
    for b in &mut pkt[3..teensy41::HEADER_SIZE] {
        *b = 0;
    }
    pkt[teensy41::HEADER_SIZE..].copy_from_slice(data);
    report
}

pub fn build_boot_report_teensy41() -> Vec<u8> {
    let mut report = vec![0u8; teensy41::PACKET_SIZE + 1];
    let pkt = &mut report[1..];
    pkt[0] = 0xFF;
    pkt[1] = 0xFF;
    pkt[2] = 0xFF;
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_block_report_format() {
        let block_addr = 0x0012_3400usize;
        let mut data = vec![0u8; teensy41::BLOCK_SIZE];
        data[0] = 0xAA;
        data[1] = 0xBB;
        data[teensy41::BLOCK_SIZE - 1] = 0xCC;

        let report = build_block_report_teensy41(block_addr, &data);
        assert_eq!(report.len(), teensy41::PACKET_SIZE + 1);
        assert_eq!(report[0], 0);

        // addr bytes (little endian 24-bit)
        assert_eq!(report[1], 0x00);
        assert_eq!(report[2], 0x34);
        assert_eq!(report[3], 0x12);

        // padding is zero
        for b in &report[4..(1 + teensy41::HEADER_SIZE)] {
            assert_eq!(*b, 0);
        }

        // payload
        let payload = &report[(1 + teensy41::HEADER_SIZE)..];
        assert_eq!(payload.len(), teensy41::BLOCK_SIZE);
        assert_eq!(payload[0], 0xAA);
        assert_eq!(payload[1], 0xBB);
        assert_eq!(payload[teensy41::BLOCK_SIZE - 1], 0xCC);
    }

    #[test]
    fn test_boot_report_format() {
        let report = build_boot_report_teensy41();
        assert_eq!(report.len(), teensy41::PACKET_SIZE + 1);
        assert_eq!(report[0], 0);
        assert_eq!(report[1], 0xFF);
        assert_eq!(report[2], 0xFF);
        assert_eq!(report[3], 0xFF);
        for b in &report[4..] {
            assert_eq!(*b, 0);
        }
    }
}
