use std::time::{Duration, Instant};

use thiserror::Error;

use crate::hex::FirmwareImage;
use crate::teensy41;

#[cfg(not(windows))]
use hidapi::{HidApi, HidDevice};

#[cfg(windows)]
use hidapi::HidApi;

#[cfg(windows)]
use std::ffi::OsStr;

#[cfg(windows)]
use std::iter;

#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;

#[cfg(windows)]
use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE,
    WAIT_OBJECT_0, WAIT_TIMEOUT,
};

#[cfg(windows)]
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, WriteFile, FILE_FLAG_OVERLAPPED, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};

#[cfg(windows)]
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};

#[cfg(windows)]
use windows_sys::Win32::System::Threading::{CreateEventW, ResetEvent, WaitForSingleObject};

#[derive(serde::Serialize)]
pub struct HalfKayDeviceSummary {
    pub vid: u16,
    pub pid: u16,
    pub path: String,
}

pub struct HalfKayDevice {
    #[cfg(not(windows))]
    _api: HidApi,
    #[cfg(not(windows))]
    dev: HidDevice,

    #[cfg(windows)]
    handle: HANDLE,
    #[cfg(windows)]
    event: HANDLE,

    pub path: String,
}

#[derive(Error, Debug)]
pub enum HalfKayError {
    #[error("hid: {0}")]
    Hid(#[from] hidapi::HidError),

    #[cfg(windows)]
    #[error("win32: {msg} (err={code})")]
    Win32 { msg: &'static str, code: u32 },

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

pub fn open_halfkay_device(
    wait: bool,
    wait_timeout: Option<Duration>,
) -> Result<HalfKayDevice, HalfKayError> {
    let start = Instant::now();
    loop {
        let api = HidApi::new()?;

        let path = api
            .device_list()
            .find(|d| d.vendor_id() == teensy41::VID && d.product_id() == teensy41::PID_HALFKAY)
            .map(|d| d.path().to_string_lossy().to_string());

        if let Some(path) = path {
            #[cfg(not(windows))]
            {
                let dev = api.open(teensy41::VID, teensy41::PID_HALFKAY)?;
                return Ok(HalfKayDevice {
                    _api: api,
                    dev,
                    path,
                });
            }

            #[cfg(windows)]
            {
                let (handle, event) = win32_open_hid_path(&path)?;
                return Ok(HalfKayDevice {
                    handle,
                    event,
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
    dev: &HalfKayDevice,
    fw: &FirmwareImage,
    block_addr: usize,
    write_index: usize,
) -> Result<(), HalfKayError> {
    let end = block_addr + teensy41::BLOCK_SIZE;
    let report = build_block_report_teensy41(block_addr, &fw.data[block_addr..end]);

    #[cfg(not(windows))]
    {
        let _ = dev.dev.write(&report)?;
        return Ok(());
    }

    #[cfg(windows)]
    {
        // Match PJRC teensy_loader_cli behavior:
        // - first few blocks may take a long time (erase)
        // - later blocks should be fast
        let total_timeout_ms = if write_index <= 4 { 45_000 } else { 500 };
        win32_write_report(dev.handle, dev.event, &report, total_timeout_ms)
    }
}

pub fn boot_teensy41(dev: &HalfKayDevice) -> Result<(), HalfKayError> {
    let report = build_boot_report_teensy41();

    // Best-effort: boot may happen immediately and invalidate the handle.
    #[cfg(not(windows))]
    {
        let _ = dev.dev.write(&report);
        Ok(())
    }

    #[cfg(windows)]
    {
        let _ = win32_write_report(dev.handle, dev.event, &report, 500);
        Ok(())
    }
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

#[cfg(windows)]
fn win32_open_hid_path(path: &str) -> Result<(HANDLE, HANDLE), HalfKayError> {
    let wide: Vec<u16> = OsStr::new(path)
        .encode_wide()
        .chain(iter::once(0))
        .collect();

    // Manual-reset event, initial state signaled (matches PJRC teensy_loader_cli).
    let event = unsafe { CreateEventW(std::ptr::null(), 1, 1, std::ptr::null()) };
    if event == 0 {
        return Err(HalfKayError::Win32 {
            msg: "CreateEventW",
            code: unsafe { GetLastError() },
        });
    }

    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_OVERLAPPED,
            0,
        )
    };

    if handle == INVALID_HANDLE_VALUE {
        unsafe { CloseHandle(event) };
        return Err(HalfKayError::Win32 {
            msg: "CreateFileW",
            code: unsafe { GetLastError() },
        });
    }

    Ok((handle, event))
}

#[cfg(windows)]
fn win32_write_report(
    handle: HANDLE,
    event: HANDLE,
    report: &[u8],
    total_timeout_ms: u32,
) -> Result<(), HalfKayError> {
    let start = Instant::now();
    let mut last_msg: &'static str = "WriteFile timeout";
    let mut last_code: u32 = WAIT_TIMEOUT;

    loop {
        let elapsed_ms: u32 = start.elapsed().as_millis().try_into().unwrap_or(u32::MAX);
        if elapsed_ms >= total_timeout_ms {
            return Err(HalfKayError::Win32 {
                msg: last_msg,
                code: last_code,
            });
        }

        let remaining_ms = total_timeout_ms - elapsed_ms;
        match win32_write_report_once(handle, event, report, remaining_ms) {
            Ok(()) => return Ok(()),
            Err(HalfKayError::Win32 { msg, code }) => {
                last_msg = msg;
                last_code = code;
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(e) => return Err(e),
        }
    }
}

#[cfg(windows)]
fn win32_write_report_once(
    handle: HANDLE,
    event: HANDLE,
    report: &[u8],
    timeout_ms: u32,
) -> Result<(), HalfKayError> {
    unsafe {
        ResetEvent(event);
        let mut ov: OVERLAPPED = std::mem::zeroed();
        ov.hEvent = event;

        let ok = WriteFile(
            handle,
            report.as_ptr() as _,
            report.len() as u32,
            std::ptr::null_mut(),
            &mut ov as *mut OVERLAPPED,
        );

        if ok == 0 {
            let err = GetLastError();
            // ERROR_IO_PENDING = 997
            if err != 997 {
                return Err(HalfKayError::Win32 {
                    msg: "WriteFile",
                    code: err,
                });
            }

            let r = WaitForSingleObject(event, timeout_ms);
            if r == WAIT_TIMEOUT {
                let _ = CancelIoEx(handle, &mut ov as *mut OVERLAPPED);
                return Err(HalfKayError::Win32 {
                    msg: "WriteFile timeout",
                    code: WAIT_TIMEOUT,
                });
            }
            if r != WAIT_OBJECT_0 {
                return Err(HalfKayError::Win32 {
                    msg: "WaitForSingleObject",
                    code: r,
                });
            }
        }

        let mut n: u32 = 0;
        let ok2 = GetOverlappedResult(handle, &mut ov as *mut OVERLAPPED, &mut n, 0);
        if ok2 == 0 {
            return Err(HalfKayError::Win32 {
                msg: "GetOverlappedResult",
                code: GetLastError(),
            });
        }
        if n == 0 {
            return Err(HalfKayError::Win32 {
                msg: "short write",
                code: 0,
            });
        }
        Ok(())
    }
}

#[cfg(windows)]
impl Drop for HalfKayDevice {
    fn drop(&mut self) {
        unsafe {
            if self.handle != 0 && self.handle != INVALID_HANDLE_VALUE {
                let _ = CloseHandle(self.handle);
            }
            if self.event != 0 {
                let _ = CloseHandle(self.event);
            }
        }
    }
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
