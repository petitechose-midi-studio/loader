use std::ffi::OsStr;
use std::iter;
use std::os::windows::ffi::OsStrExt;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE,
    WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, WriteFile, FILE_FLAG_OVERLAPPED, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::System::Diagnostics::Debug::{
    FormatMessageW, FORMAT_MESSAGE_FROM_SYSTEM, FORMAT_MESSAGE_IGNORE_INSERTS,
};
use windows_sys::Win32::System::Threading::{CreateEventW, ResetEvent, WaitForSingleObject};
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};

use super::HalfKayError;

pub struct Win32HalfKayDevice {
    handle: HANDLE,
    event: HANDLE,
}

impl Win32HalfKayDevice {
    pub fn open_hid_path(path: &str) -> Result<Self, HalfKayError> {
        let wide: Vec<u16> = OsStr::new(path)
            .encode_wide()
            .chain(iter::once(0))
            .collect();

        // Manual-reset event, initial state signaled (matches PJRC teensy_loader_cli).
        let event = unsafe { CreateEventW(std::ptr::null(), 1, 1, std::ptr::null()) };
        if event == 0 {
            return Err(last_error("CreateEventW"));
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
            return Err(last_error("CreateFileW"));
        }

        Ok(Self { handle, event })
    }

    pub fn write_report(&self, report: &[u8], total_timeout_ms: u32) -> Result<(), HalfKayError> {
        let start = Instant::now();
        let mut last_err: HalfKayError = win32_error("WriteFile timeout", WAIT_TIMEOUT);

        loop {
            let elapsed_ms: u32 = start.elapsed().as_millis().try_into().unwrap_or(u32::MAX);
            if elapsed_ms >= total_timeout_ms {
                return Err(last_err);
            }

            let remaining_ms = total_timeout_ms - elapsed_ms;
            match write_report_once(self.handle, self.event, report, remaining_ms) {
                Ok(()) => return Ok(()),
                Err(e) => {
                    last_err = e;
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        }
    }
}

impl Drop for Win32HalfKayDevice {
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

fn write_report_once(
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
                return Err(win32_error("WriteFile", err));
            }

            let r = WaitForSingleObject(event, timeout_ms);
            if r == WAIT_TIMEOUT {
                // Cancel is asynchronous. We must wait for completion before returning,
                // otherwise the kernel may still access our stack-allocated OVERLAPPED.
                let _ = CancelIoEx(handle, &mut ov as *mut OVERLAPPED);

                let mut _n_cancel: u32 = 0;
                let _ = GetOverlappedResult(handle, &mut ov as *mut OVERLAPPED, &mut _n_cancel, 1);

                return Err(win32_error("WriteFile timeout", WAIT_TIMEOUT));
            }
            if r != WAIT_OBJECT_0 {
                if r == WAIT_FAILED {
                    return Err(last_error("WaitForSingleObject"));
                }
                return Err(win32_error("WaitForSingleObject", r));
            }
        }

        let mut n: u32 = 0;
        let ok2 = GetOverlappedResult(handle, &mut ov as *mut OVERLAPPED, &mut n, 0);
        if ok2 == 0 {
            return Err(last_error("GetOverlappedResult"));
        }
        if n == 0 {
            return Err(win32_error("short write", 0));
        }
        Ok(())
    }
}

fn last_error(msg: &'static str) -> HalfKayError {
    win32_error(msg, unsafe { GetLastError() })
}

fn win32_error(msg: &'static str, code: u32) -> HalfKayError {
    HalfKayError::Win32 {
        msg,
        code,
        detail: format_win32_error(code),
    }
}

fn format_win32_error(code: u32) -> String {
    let mut buf = [0u16; 512];
    let n = unsafe {
        FormatMessageW(
            FORMAT_MESSAGE_FROM_SYSTEM | FORMAT_MESSAGE_IGNORE_INSERTS,
            std::ptr::null(),
            code,
            0,
            buf.as_mut_ptr(),
            buf.len() as u32,
            std::ptr::null(),
        )
    };
    if n == 0 {
        return "unknown error".to_string();
    }
    String::from_utf16_lossy(&buf[..(n as usize)])
        .trim()
        .to_string()
}
