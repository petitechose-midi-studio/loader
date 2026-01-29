pub const VID: u16 = 0x16C0;
pub const PID_HALFKAY: u16 = 0x0478;

pub const CODE_SIZE: usize = 8_126_464;
pub const BLOCK_SIZE: usize = 1024;
pub const HEADER_SIZE: usize = 64;
pub const PACKET_SIZE: usize = HEADER_SIZE + BLOCK_SIZE; // 1088

pub const FLEXSPI_BASE: u32 = 0x6000_0000;
