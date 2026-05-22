//! ICMP プロトコル
//!
//! - Echo Request を受信して Echo Reply を返す (ping 応答)
//! - Destination Unreachable 等のエラーメッセージ送信

use crate::ip::{self, IpAddr, IpHdr, IpIface, IP_PROTOCOL_ICMP};
use crate::util;

pub const ICMP_HDR_SIZE: usize = 8;

// Type
pub const ICMP_TYPE_ECHO_REPLY: u8 = 0;
pub const ICMP_TYPE_DEST_UNREACH: u8 = 3;
pub const ICMP_TYPE_REDIRECT: u8 = 5;
pub const ICMP_TYPE_ECHO: u8 = 8;
pub const ICMP_TYPE_TIME_EXCEEDED: u8 = 11;

// Code (Destination Unreachable)
pub const ICMP_CODE_NET_UNREACH: u8 = 0;
pub const ICMP_CODE_HOST_UNREACH: u8 = 1;
pub const ICMP_CODE_PROTO_UNREACH: u8 = 2;
pub const ICMP_CODE_PORT_UNREACH: u8 = 3;

/// ICMP 共通ヘッダのゼロコピーパーサ。
pub struct IcmpCommon<'a> {
    data: &'a [u8],
}

impl<'a> IcmpCommon<'a> {
    pub fn new(data: &'a [u8]) -> Option<Self> {
        if data.len() < ICMP_HDR_SIZE {
            return None;
        }
        Some(Self { data })
    }
    pub fn ty(&self) -> u8 {
        self.data[0]
    }
    pub fn code(&self) -> u8 {
        self.data[1]
    }
    pub fn sum(&self) -> u16 {
        u16::from_be_bytes([self.data[2], self.data[3]])
    }
    /// type-dependent field (Echo: id+seq, DestUnreach: unused+nexthop_mtu)
    pub fn dep(&self) -> u32 {
        u32::from_be_bytes([self.data[4], self.data[5], self.data[6], self.data[7]])
    }
}

/// ICMP Echo のパーサ。
pub struct IcmpEcho<'a> {
    data: &'a [u8],
}

impl<'a> IcmpEcho<'a> {
    pub fn new(data: &'a [u8]) -> Option<Self> {
        if data.len() < ICMP_HDR_SIZE {
            return None;
        }
        Some(Self { data })
    }
    pub fn id(&self) -> u16 {
        u16::from_be_bytes([self.data[4], self.data[5]])
    }
    pub fn seq(&self) -> u16 {
        u16::from_be_bytes([self.data[6], self.data[7]])
    }
}

fn type_name(ty: u8) -> &'static str {
    match ty {
        ICMP_TYPE_ECHO_REPLY => "EchoReply",
        ICMP_TYPE_DEST_UNREACH => "DestinationUnreachable",
        ICMP_TYPE_REDIRECT => "Redirect",
        ICMP_TYPE_ECHO => "Echo",
        ICMP_TYPE_TIME_EXCEEDED => "TimeExceeded",
        _ => "Unknown",
    }
}

/// ICMP パケットを送信する。
pub fn output(
    ty: u8,
    code: u8,
    values: u32,
    data: &[u8],
    src: IpAddr,
    dst: IpAddr,
) -> Result<(), ()> {
    let total = ICMP_HDR_SIZE + data.len();
    let mut buf = vec![0u8; total];
    buf[0] = ty;
    buf[1] = code;
    buf[4..8].copy_from_slice(&values.to_be_bytes());
    buf[ICMP_HDR_SIZE..].copy_from_slice(data);
    let sum = util::cksum16(&buf, 0);
    buf[2..4].copy_from_slice(&sum.to_ne_bytes());

    log::debug!(
        "icmp: output: {} => {}, type={} ({}), len={}",
        src,
        dst,
        ty,
        type_name(ty),
        total
    );
    ip::output(IP_PROTOCOL_ICMP, &buf, src, dst)
}

/// ICMP 入力ハンドラ。IP 層から呼ばれる。
fn input(hdr: &IpHdr<'_>, data: &[u8], iface: &IpIface) {
    log::debug!(
        "icmp: input: {} => {}, dev={}, len={}",
        hdr.src(),
        hdr.dst(),
        iface.dev().name,
        data.len()
    );

    if data.len() < ICMP_HDR_SIZE {
        log::error!("icmp: too short: len={}", data.len());
        return;
    }
    if util::cksum16(data, 0) != 0 {
        log::error!("icmp: checksum error");
        return;
    }

    let com = IcmpCommon::new(data).unwrap();
    log::debug!(
        "icmp: type={} ({}), code={}",
        com.ty(),
        type_name(com.ty()),
        com.code()
    );

    // Echo Request → Echo Reply を返す
    if com.ty() == ICMP_TYPE_ECHO {
        let echo = IcmpEcho::new(data).unwrap();
        log::info!("icmp: Echo Request: id={}, seq={}", echo.id(), echo.seq());
        let _ = output(
            ICMP_TYPE_ECHO_REPLY,
            0,
            com.dep(), // id + seq をそのまま返す
            &data[ICMP_HDR_SIZE..],
            iface.unicast(),
            hdr.src(),
        );
    }
}

pub fn init() -> Result<(), ()> {
    ip::register_protocol(IP_PROTOCOL_ICMP, input)?;
    Ok(())
}
