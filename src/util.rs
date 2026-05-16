//! ユーティリティ関数。
//!
//! - `cksum16`: RFC 1071 インターネットチェックサム
//! - `HexDump`: パケットの16進ダンプ表示

use std::fmt;

/// RFC 1071 インターネットチェックサム。
/// IP, ICMP, TCP, UDP のヘッダ/データ検証に使う。
///
/// `init` に前半の部分和 (ビット反転前) を渡すと、
/// 分割計算ができる。
pub fn cksum16(data: &[u8], init: u32) -> u16 {
    let mut sum = init;
    let mut i = 0;
    while i + 1 < data.len() {
        sum += u16::from_ne_bytes([data[i], data[i + 1]]) as u32;
        i += 2;
    }
    if i < data.len() {
        sum += data[i] as u32;
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

/// バイト列を見やすい16進ダンプとして表示するフォーマッタ。
pub struct HexDump<'a>(pub &'a [u8]);

impl fmt::Display for HexDump<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let data = self.0;
        writeln!(f, "+------+-------------------------------------------------+------------------+")?;
        for (offset, chunk) in data.chunks(16).enumerate() {
            write!(f, "| {:04x} | ", offset * 16)?;
            for i in 0..16 {
                if i < chunk.len() {
                    write!(f, "{:02x} ", chunk[i])?;
                } else {
                    write!(f, "   ")?;
                }
            }
            write!(f, "| ")?;
            for i in 0..16 {
                if i < chunk.len() {
                    let b = chunk[i];
                    if b.is_ascii_graphic() || b == b' ' {
                        write!(f, "{}", b as char)?;
                    } else {
                        write!(f, ".")?;
                    }
                } else {
                    write!(f, " ")?;
                }
            }
            writeln!(f, " |")?;
        }
        write!(f, "+------+-------------------------------------------------+------------------+")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // microps-rs のテストデータ: IP(20B) + ICMP(28B)
    const TEST_DATA: &[u8] = &[
        0x45, 0x00, 0x00, 0x30, 0x00, 0x80, 0x00, 0x00,
        0xff, 0x01, 0xbd, 0x4a, 0x7f, 0x00, 0x00, 0x01,
        0x7f, 0x00, 0x00, 0x01, 0x08, 0x00, 0x35, 0x64,
        0x00, 0x80, 0x00, 0x01, 0x31, 0x32, 0x33, 0x34,
        0x35, 0x36, 0x37, 0x38, 0x39, 0x30, 0x21, 0x40,
        0x23, 0x24, 0x25, 0x5e, 0x26, 0x2a, 0x28, 0x29,
    ];

    #[test]
    fn verify_ip_header_checksum() {
        assert_eq!(cksum16(&TEST_DATA[..20], 0), 0);
    }

    #[test]
    fn verify_icmp_checksum() {
        assert_eq!(cksum16(&TEST_DATA[20..], 0), 0);
    }

    #[test]
    fn compute_checksum() {
        let mut hdr = [0u8; 20];
        hdr.copy_from_slice(&TEST_DATA[..20]);
        hdr[10] = 0;
        hdr[11] = 0;
        let computed = cksum16(&hdr, 0);
        let original = u16::from_ne_bytes([TEST_DATA[10], TEST_DATA[11]]);
        assert_eq!(computed, original);
    }

    #[test]
    fn checksum_empty() {
        assert_eq!(cksum16(&[], 0), 0xffff);
    }

    #[test]
    fn checksum_split() {
        let part1 = &TEST_DATA[..10];
        let part2 = &TEST_DATA[10..20];
        let partial = cksum16(part1, 0);
        let combined = cksum16(part2, !partial as u32);
        let whole = cksum16(&TEST_DATA[..20], 0);
        assert_eq!(combined, whole);
    }
}
