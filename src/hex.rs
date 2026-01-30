use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::path::Path;

use thiserror::Error;

use crate::teensy41;

pub struct FirmwareImage {
    pub data: Vec<u8>,
    pub byte_count: usize,
    pub num_blocks: usize,
    pub blocks_to_write: Vec<usize>,
}

impl FirmwareImage {
    pub fn load_teensy41(path: &Path) -> Result<Self, HexError> {
        let mut data = vec![0xFFu8; teensy41::CODE_SIZE];
        let mut mask = vec![false; teensy41::CODE_SIZE];
        let mut byte_count: usize = 0;

        let f = File::open(path).map_err(HexError::Io)?;
        let r = BufReader::new(f);

        let mut ext_addr: u32 = 0;

        for (line_no, line) in r.lines().enumerate() {
            let line_no = line_no + 1;
            let line = match line {
                Ok(s) => s,
                Err(e) if e.kind() == io::ErrorKind::InvalidData => {
                    return Err(HexError::NotText { line_no });
                }
                Err(e) => return Err(HexError::Io(e)),
            };
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if !line.starts_with(':') {
                return Err(HexError::InvalidLine {
                    line_no,
                    msg: "missing ':' prefix".to_string(),
                });
            }

            let bytes = decode_hex_bytes(&line[1..])
                .map_err(|e| HexError::InvalidLine { line_no, msg: e })?;

            if bytes.len() < 5 {
                return Err(HexError::InvalidLine {
                    line_no,
                    msg: "record too short".to_string(),
                });
            }

            let len = bytes[0] as usize;
            let addr = u16::from_be_bytes([bytes[1], bytes[2]]) as u32;
            let rec_type = bytes[3];

            if bytes.len() != 5 + len {
                return Err(HexError::InvalidLine {
                    line_no,
                    msg: format!(
                        "bad length (expected {} bytes after ':', got {})",
                        2 * (5 + len),
                        2 * bytes.len()
                    ),
                });
            }

            let payload = &bytes[4..4 + len];
            let checksum = bytes[4 + len];
            if checksum != checksum_ihex(&bytes[0..4 + len]) {
                return Err(HexError::InvalidChecksum { line_no });
            }

            match rec_type {
                0x00 => {
                    // data
                    byte_count = byte_count.saturating_add(len);
                    for (i, b) in payload.iter().copied().enumerate() {
                        let abs = ext_addr
                            .checked_add(addr)
                            .and_then(|v| v.checked_add(i as u32))
                            .ok_or(HexError::AddressOverflow { line_no })?;
                        let abs = map_teensy41_addr(abs)
                            .ok_or(HexError::AddressOutOfRange { line_no, addr: abs })?;
                        data[abs] = b;
                        mask[abs] = true;
                    }
                }
                0x01 => {
                    // EOF
                    break;
                }
                0x02 => {
                    // extended segment address (<< 4)
                    if len == 2 {
                        let seg = u16::from_be_bytes([payload[0], payload[1]]) as u32;
                        ext_addr = seg << 4;
                    }
                }
                0x04 => {
                    // extended linear address (<< 16)
                    if len == 2 {
                        let hi = u16::from_be_bytes([payload[0], payload[1]]) as u32;
                        ext_addr = hi << 16;
                        // Teensy 4.x HEX uses FlexSPI base (0x60000000).
                        if ext_addr >= teensy41::FLEXSPI_BASE
                            && ext_addr < teensy41::FLEXSPI_BASE + teensy41::CODE_SIZE as u32
                        {
                            ext_addr -= teensy41::FLEXSPI_BASE;
                        }
                    }
                }
                _ => {
                    // ignore other non-data records
                }
            }
        }

        let num_blocks = teensy41::CODE_SIZE / teensy41::BLOCK_SIZE;

        let mut blocks_to_write: Vec<usize> = Vec::new();
        for block_idx in 0..num_blocks {
            let start = block_idx * teensy41::BLOCK_SIZE;
            if block_idx == 0 {
                blocks_to_write.push(start);
                continue;
            }
            if !is_block_blank(&data, &mask, start) {
                blocks_to_write.push(start);
            }
        }

        Ok(Self {
            data,
            byte_count,
            num_blocks,
            blocks_to_write,
        })
    }
}

#[derive(Error, Debug)]
pub enum HexError {
    #[error("io: {0}")]
    Io(io::Error),

    #[error(
        "input is not a text Intel HEX file (invalid UTF-8 at line {line_no}); did you pass a .elf?"
    )]
    NotText { line_no: usize },

    #[error("invalid hex line {line_no}: {msg}")]
    InvalidLine { line_no: usize, msg: String },

    #[error("invalid checksum at line {line_no}")]
    InvalidChecksum { line_no: usize },

    #[error("address overflow at line {line_no}")]
    AddressOverflow { line_no: usize },

    #[error("address out of Teensy 4.1 range at line {line_no}: 0x{addr:08X}")]
    AddressOutOfRange { line_no: usize, addr: u32 },
}

fn is_block_blank(data: &[u8], mask: &[bool], start: usize) -> bool {
    let end = start + teensy41::BLOCK_SIZE;
    for i in start..end {
        if mask[i] && data[i] != 0xFF {
            return false;
        }
    }
    true
}

fn map_teensy41_addr(addr: u32) -> Option<usize> {
    // After FlexSPI mapping, valid firmware addresses are within [0, CODE_SIZE).
    let a = addr as usize;
    if a < teensy41::CODE_SIZE {
        Some(a)
    } else {
        None
    }
}

fn decode_hex_bytes(s: &str) -> Result<Vec<u8>, String> {
    if !s.len().is_multiple_of(2) {
        return Err("odd number of hex digits".to_string());
    }
    let mut out = Vec::with_capacity(s.len() / 2);
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let hi = from_hex_digit(bytes[i]).ok_or_else(|| "invalid hex digit".to_string())?;
        let lo = from_hex_digit(bytes[i + 1]).ok_or_else(|| "invalid hex digit".to_string())?;
        out.push((hi << 4) | lo);
        i += 2;
    }
    Ok(out)
}

fn from_hex_digit(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

fn checksum_ihex(bytes: &[u8]) -> u8 {
    let sum: u8 = bytes.iter().fold(0u8, |acc, b| acc.wrapping_add(*b));
    (!sum).wrapping_add(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    use tempfile::NamedTempFile;

    fn ihex_record(addr: u16, rec_type: u8, payload: &[u8]) -> String {
        let mut bytes: Vec<u8> = Vec::new();
        bytes.push(payload.len() as u8);
        bytes.extend_from_slice(&addr.to_be_bytes());
        bytes.push(rec_type);
        bytes.extend_from_slice(payload);
        let cksum = checksum_ihex(&bytes);
        bytes.push(cksum);

        let mut s = String::from(":");
        for b in bytes {
            s.push_str(&format!("{b:02X}"));
        }
        s
    }

    #[test]
    fn test_load_teensy41_maps_flexspi_base() {
        // Set extended linear address = 0x6000 -> 0x60000000 (FlexSPI base)
        let ext = ihex_record(0x0000, 0x04, &[0x60, 0x00]);
        let data = ihex_record(0x0010, 0x00, &[0xDE, 0xAD, 0xBE, 0xEF]);
        let eof = ihex_record(0x0000, 0x01, &[]);

        let content = format!("{ext}\n{data}\n{eof}\n");
        let mut f = NamedTempFile::new().unwrap();
        std::io::Write::write_all(&mut f, content.as_bytes()).unwrap();

        let fw = FirmwareImage::load_teensy41(f.path()).unwrap();
        assert_eq!(fw.data[0x10], 0xDE);
        assert_eq!(fw.data[0x11], 0xAD);
        assert_eq!(fw.data[0x12], 0xBE);
        assert_eq!(fw.data[0x13], 0xEF);
        assert!(fw.blocks_to_write.contains(&0));
    }

    #[test]
    fn test_load_teensy41_rejects_out_of_range_address() {
        // ext linear address = 0x607C -> 0x607C0000 (just beyond FlexSPI mapped range)
        let ext = ihex_record(0x0000, 0x04, &[0x60, 0x7C]);
        let data = ihex_record(0x0000, 0x00, &[0x01]);
        let eof = ihex_record(0x0000, 0x01, &[]);

        let content = format!("{ext}\n{data}\n{eof}\n");
        let mut f = NamedTempFile::new().unwrap();
        std::io::Write::write_all(&mut f, content.as_bytes()).unwrap();

        let err = match FirmwareImage::load_teensy41(f.path()) {
            Ok(_) => panic!("expected AddressOutOfRange"),
            Err(e) => e,
        };
        match err {
            HexError::AddressOutOfRange { .. } => {}
            _ => panic!("expected AddressOutOfRange, got {err:?}"),
        }
    }

    #[test]
    fn test_load_teensy41_detects_bad_checksum() {
        // Record with wrong checksum (last byte '00')
        let bad = ":04001000DEADBEEF00".to_string();
        let eof = ihex_record(0x0000, 0x01, &[]);
        let content = format!("{bad}\n{eof}\n");
        let mut f = NamedTempFile::new().unwrap();
        std::io::Write::write_all(&mut f, content.as_bytes()).unwrap();

        let err = match FirmwareImage::load_teensy41(f.path()) {
            Ok(_) => panic!("expected InvalidChecksum"),
            Err(e) => e,
        };
        match err {
            HexError::InvalidChecksum { .. } => {}
            _ => panic!("expected InvalidChecksum, got {err:?}"),
        }
    }
}
