use std::time::Duration;
#[cfg(not(windows))]
use std::time::Instant;

use hidapi::HidApi;
#[cfg(not(windows))]
use hidapi::HidDevice;
#[cfg(not(windows))]
use std::ffi::CString;
use thiserror::Error;

use crate::{hex::FirmwareImage, teensy41};

// Match PJRC teensy_loader_cli behavior:
// - first few blocks may take a long time (erase)
// - later blocks should be fast
const SLOW_BLOCK_MAX_INDEX: usize = 4;
const SLOW_BLOCK_TIMEOUT: Duration = Duration::from_secs(45);
const FAST_BLOCK_TIMEOUT: Duration = Duration::from_millis(500);
#[cfg(not(windows))]
const RETRY_SLEEP: Duration = Duration::from_millis(10);

fn block_total_timeout(write_index: usize) -> Duration {
    if write_index <= SLOW_BLOCK_MAX_INDEX {
        SLOW_BLOCK_TIMEOUT
    } else {
        FAST_BLOCK_TIMEOUT
    }
}

#[cfg(not(windows))]
fn reopen_best_effort(dev: &mut HalfKayDevice) {
    // HID handles can become unusable after a USB reset. Reopening by the same path is cheap,
    // but the kernel may re-enumerate to a different hidraw node, so fall back to scanning.
    let path = dev.path.clone();
    if let Ok(new_dev) = open_by_path(&path) {
        *dev = new_dev;
        return;
    }

    if let Ok(new_dev) = open_halfkay_device(false, None) {
        *dev = new_dev;
    }
}

#[cfg(windows)]
mod win32;

#[cfg(windows)]
use win32::Win32HalfKayDevice;

#[derive(serde::Serialize)]
pub struct HalfKayDeviceSummary {
    pub vid: u16,
    pub pid: u16,
    pub path: String,
}

pub struct HalfKayDevice {
    backend: Backend,
    pub path: String,
}

enum Backend {
    #[cfg(not(windows))]
    HidApi(HidDevice),
    #[cfg(windows)]
    Win32(Win32HalfKayDevice),
}

#[derive(Error, Debug)]
pub enum HalfKayError {
    #[error("hid: {0}")]
    Hid(#[from] hidapi::HidError),

    #[cfg(windows)]
    #[error("win32: {msg} (err={code}): {detail}")]
    Win32 {
        msg: &'static str,
        code: u32,
        detail: String,
    },

    #[error("short write: {got} != {expected}")]
    ShortWrite { got: usize, expected: usize },

    #[error("invalid HID path")]
    InvalidPath,

    #[error("no HalfKay device found")]
    NoDevice,
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

pub fn list_paths() -> Result<Vec<String>, HalfKayError> {
    let api = HidApi::new()?;
    let mut out: Vec<String> = Vec::new();
    for d in api.device_list() {
        if d.vendor_id() == teensy41::VID && d.product_id() == teensy41::PID_HALFKAY {
            out.push(d.path().to_string_lossy().to_string());
        }
    }
    out.sort();
    Ok(out)
}

pub fn open_by_path(path: &str) -> Result<HalfKayDevice, HalfKayError> {
    #[cfg(not(windows))]
    {
        let api = HidApi::new()?;
        let cpath = CString::new(path).map_err(|_| HalfKayError::InvalidPath)?;
        let dev = api.open_path(&cpath)?;
        Ok(HalfKayDevice {
            backend: Backend::HidApi(dev),
            path: path.to_string(),
        })
    }

    #[cfg(windows)]
    {
        let dev = Win32HalfKayDevice::open_hid_path(path)?;
        Ok(HalfKayDevice {
            backend: Backend::Win32(dev),
            path: path.to_string(),
        })
    }
}

pub fn open_halfkay_device(
    wait: bool,
    wait_timeout: Option<Duration>,
) -> Result<HalfKayDevice, HalfKayError> {
    let start = Instant::now();
    loop {
        let api = HidApi::new()?;

        let dev = api
            .device_list()
            .find(|d| d.vendor_id() == teensy41::VID && d.product_id() == teensy41::PID_HALFKAY);

        if let Some(dev) = dev {
            let path = dev.path().to_string_lossy().to_string();

            #[cfg(not(windows))]
            {
                let dev = api.open_path(dev.path())?;
                return Ok(HalfKayDevice {
                    backend: Backend::HidApi(dev),
                    path,
                });
            }

            #[cfg(windows)]
            {
                let dev = Win32HalfKayDevice::open_hid_path(&path)?;
                return Ok(HalfKayDevice {
                    backend: Backend::Win32(dev),
                    path,
                });
            }
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
    dev: &mut HalfKayDevice,
    fw: &FirmwareImage,
    block_addr: usize,
    write_index: usize,
) -> Result<(), HalfKayError> {
    let end = block_addr + teensy41::BLOCK_SIZE;
    let mut report = [0u8; teensy41::PACKET_SIZE + 1];
    fill_block_report_teensy41(&mut report, block_addr, &fw.data[block_addr..end]);

    match &dev.backend {
        #[cfg(not(windows))]
        Backend::HidApi(_) => {
            let timeout = block_total_timeout(write_index);
            let start = Instant::now();
            let mut last_reopen = Instant::now();

            loop {
                // Keep borrows short so we can best-effort reopen between attempts.
                let r = match &dev.backend {
                    Backend::HidApi(h) => h.write(&report).map_err(HalfKayError::Hid),
                    #[allow(unreachable_patterns)]
                    _ => return Err(HalfKayError::NoDevice),
                };
                match r {
                    Ok(n) => {
                        if n != report.len() {
                            return Err(HalfKayError::ShortWrite {
                                got: n,
                                expected: report.len(),
                            });
                        }
                        return Ok(());
                    }
                    Err(err) => {
                        if start.elapsed() >= timeout {
                            return Err(err);
                        }

                        // If the HID pipe broke, reopening often recovers immediately.
                        // Throttle reopen attempts to avoid making success cases slower.
                        let is_broken_pipe = err.to_string().contains("Broken pipe");
                        if is_broken_pipe && last_reopen.elapsed() >= Duration::from_millis(100) {
                            reopen_best_effort(dev);
                            last_reopen = Instant::now();
                        }

                        std::thread::sleep(RETRY_SLEEP);
                    }
                }
            }
        }

        #[cfg(windows)]
        Backend::Win32(h) => {
            let total_timeout_ms: u32 = block_total_timeout(write_index)
                .as_millis()
                .try_into()
                .unwrap_or(u32::MAX);
            h.write_report(&report, total_timeout_ms)
        }
    }
}

pub fn boot_teensy41(dev: &mut HalfKayDevice) -> Result<(), HalfKayError> {
    let mut report = [0u8; teensy41::PACKET_SIZE + 1];
    fill_boot_report_teensy41(&mut report);

    #[cfg(not(windows))]
    {
        let timeout = FAST_BLOCK_TIMEOUT;
        let start = Instant::now();
        let mut last_reopen = Instant::now();

        loop {
            let r = match &dev.backend {
                Backend::HidApi(h) => h.write(&report).map_err(HalfKayError::Hid),
                #[allow(unreachable_patterns)]
                _ => return Ok(()),
            };

            match r {
                Ok(_) => return Ok(()),
                Err(err) => {
                    if start.elapsed() >= timeout {
                        return Ok(());
                    }

                    let is_broken_pipe = err.to_string().contains("Broken pipe");
                    if is_broken_pipe && last_reopen.elapsed() >= Duration::from_millis(100) {
                        reopen_best_effort(dev);
                        last_reopen = Instant::now();
                    }
                    std::thread::sleep(RETRY_SLEEP);
                }
            }
        }
    }

    #[cfg(windows)]
    {
        // Best-effort: boot may happen immediately and invalidate the handle.
        match &dev.backend {
            Backend::Win32(h) => {
                let _ = h.write_report(&report, 500);
            }
        }
        Ok(())
    }
}

pub fn fill_block_report_teensy41(
    report: &mut [u8; teensy41::PACKET_SIZE + 1],
    block_addr: usize,
    data: &[u8],
) {
    assert_eq!(data.len(), teensy41::BLOCK_SIZE);

    // First byte is Report ID (0).
    report.fill(0);

    let pkt = &mut report[1..];
    let addr = block_addr as u32;
    pkt[0] = (addr & 0xFF) as u8;
    pkt[1] = ((addr >> 8) & 0xFF) as u8;
    pkt[2] = ((addr >> 16) & 0xFF) as u8;
    pkt[3..teensy41::HEADER_SIZE].fill(0);
    pkt[teensy41::HEADER_SIZE..].copy_from_slice(data);
}

pub fn fill_boot_report_teensy41(report: &mut [u8; teensy41::PACKET_SIZE + 1]) {
    report.fill(0);
    let pkt = &mut report[1..];
    pkt[0] = 0xFF;
    pkt[1] = 0xFF;
    pkt[2] = 0xFF;
}

pub fn build_block_report_teensy41(block_addr: usize, data: &[u8]) -> Vec<u8> {
    let mut report = [0u8; teensy41::PACKET_SIZE + 1];
    fill_block_report_teensy41(&mut report, block_addr, data);
    report.to_vec()
}

pub fn build_boot_report_teensy41() -> Vec<u8> {
    let mut report = [0u8; teensy41::PACKET_SIZE + 1];
    fill_boot_report_teensy41(&mut report);
    report.to_vec()
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

    #[test]
    fn test_block_total_timeout_matches_pjrc_policy() {
        for i in 0..=SLOW_BLOCK_MAX_INDEX {
            assert_eq!(block_total_timeout(i), SLOW_BLOCK_TIMEOUT);
        }
        assert_eq!(
            block_total_timeout(SLOW_BLOCK_MAX_INDEX + 1),
            FAST_BLOCK_TIMEOUT
        );
        assert_eq!(block_total_timeout(9999), FAST_BLOCK_TIMEOUT);
    }
}
