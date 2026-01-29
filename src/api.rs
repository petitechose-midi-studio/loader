use std::path::Path;
use std::time::Duration;

use thiserror::Error;

use crate::{halfkay, hex, serial_reboot, teensy41};

#[derive(Debug, Clone)]
pub struct FlashOptions {
    pub wait: bool,
    pub wait_timeout: Option<Duration>,
    pub no_reboot: bool,
    pub retries: u32,
    pub soft_reboot: bool,
    pub serial_port: Option<String>,
    pub reopen_timeout: Duration,
    pub reopen_delay: Duration,
    pub soft_reboot_delay: Duration,
}

impl Default for FlashOptions {
    fn default() -> Self {
        Self {
            wait: false,
            wait_timeout: None,
            no_reboot: false,
            retries: 3,
            soft_reboot: false,
            serial_port: None,
            reopen_timeout: Duration::from_secs(10),
            reopen_delay: Duration::from_millis(150),
            soft_reboot_delay: Duration::from_millis(250),
        }
    }
}

#[derive(Debug, Clone)]
pub enum FlashEvent {
    HexLoaded {
        bytes: usize,
        blocks: usize,
    },
    SoftReboot {
        port: String,
    },
    SoftRebootSkipped {
        error: String,
    },
    HalfKayOpen {
        path: String,
    },
    Block {
        index: usize,
        total: usize,
        addr: usize,
    },
    Retry {
        addr: usize,
        attempt: u32,
        retries: u32,
        error: String,
    },
    Boot,
    Done,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum FlashErrorKind {
    NoDevice,
    InvalidHex,
    WriteFailed,
}

#[derive(Error, Debug)]
pub enum FlashError {
    #[error("unable to open HalfKay device (VID:PID {vid:04X}:{pid:04X}): {source}")]
    NoDevice {
        vid: u16,
        pid: u16,
        #[source]
        source: halfkay::HalfKayError,
    },

    #[error("invalid HEX: {source}")]
    InvalidHex {
        #[source]
        source: hex::HexError,
    },

    #[error("write failed at addr=0x{addr:06X} after {attempts} attempts: {source}")]
    WriteFailed {
        addr: usize,
        attempts: u32,
        #[source]
        source: halfkay::HalfKayError,
    },

    #[error("unable to reopen HalfKay device while writing addr=0x{addr:06X}: {source}")]
    ReopenFailed {
        addr: usize,
        #[source]
        source: halfkay::HalfKayError,
    },
}

impl FlashError {
    pub fn kind(&self) -> FlashErrorKind {
        match self {
            FlashError::NoDevice { .. } => FlashErrorKind::NoDevice,
            FlashError::InvalidHex { .. } => FlashErrorKind::InvalidHex,
            FlashError::WriteFailed { .. } | FlashError::ReopenFailed { .. } => {
                FlashErrorKind::WriteFailed
            }
        }
    }
}

pub fn flash_teensy41<F>(
    hex_path: &Path,
    opts: &FlashOptions,
    mut on_event: F,
) -> Result<(), FlashError>
where
    F: FnMut(FlashEvent),
{
    let fw = hex::FirmwareImage::load_teensy41(hex_path)
        .map_err(|e| FlashError::InvalidHex { source: e })?;
    on_event(FlashEvent::HexLoaded {
        bytes: fw.byte_count,
        blocks: fw.num_blocks,
    });

    let mut dev = halfkay::open_halfkay_device(false, None)
        .or_else(|e| {
            if !matches!(e, halfkay::HalfKayError::NoDevice) {
                return Err(e);
            }

            // Best-effort: if the caller asked to wait, try entering HalfKay without the button.
            if opts.wait || opts.soft_reboot {
                match serial_reboot::soft_reboot_teensy41(opts.serial_port.as_deref()) {
                    Ok(port) => {
                        on_event(FlashEvent::SoftReboot { port });
                        std::thread::sleep(opts.soft_reboot_delay);
                    }
                    Err(e) => {
                        on_event(FlashEvent::SoftRebootSkipped {
                            error: e.to_string(),
                        });
                    }
                }
            }

            let wait = opts.wait || opts.soft_reboot;
            halfkay::open_halfkay_device(wait, opts.wait_timeout)
        })
        .map_err(|e| FlashError::NoDevice {
            vid: teensy41::VID,
            pid: teensy41::PID_HALFKAY,
            source: e,
        })?;

    on_event(FlashEvent::HalfKayOpen {
        path: dev.path.clone(),
    });

    let total_to_write = fw.blocks_to_write.len();
    for (i, block_addr) in fw.blocks_to_write.iter().copied().enumerate() {
        on_event(FlashEvent::Block {
            index: i,
            total: total_to_write,
            addr: block_addr,
        });

        let mut attempt: u32 = 0;
        loop {
            attempt = attempt.saturating_add(1);
            match halfkay::write_block_teensy41(&dev, &fw, block_addr, i) {
                Ok(()) => break,
                Err(e) => {
                    if attempt > opts.retries {
                        return Err(FlashError::WriteFailed {
                            addr: block_addr,
                            attempts: attempt,
                            source: e,
                        });
                    }

                    on_event(FlashEvent::Retry {
                        addr: block_addr,
                        attempt,
                        retries: opts.retries,
                        error: e.to_string(),
                    });

                    std::thread::sleep(opts.reopen_delay);
                    dev = halfkay::open_halfkay_device(true, Some(opts.reopen_timeout)).map_err(
                        |e2| FlashError::ReopenFailed {
                            addr: block_addr,
                            source: e2,
                        },
                    )?;
                    std::thread::sleep(opts.reopen_delay);
                }
            }
        }
    }

    if !opts.no_reboot {
        on_event(FlashEvent::Boot);
        let _ = halfkay::boot_teensy41(&dev);
    }

    on_event(FlashEvent::Done);
    Ok(())
}
