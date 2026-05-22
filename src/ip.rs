//! IP プロトコル
//!
//! - IpAddr: IPv4 アドレス (4バイト)
//! - IpHdr: IP ヘッダのゼロコピーパーサ
//! - IpIface: IP 論理インタフェース (unicast, netmask, broadcast)
//! - ルーティングテーブル
//! - IP パケットの入出力

use std::any::Any;
use std::fmt;
use std::str::FromStr;
use std::sync::{Arc, Mutex};

use crate::device::{Device, NetIface, ADDR_LEN, FAMILY_IP, FLAG_NEED_ARP};
use crate::net;
use crate::util;

pub const IP_ADDR_LEN: usize = 4;
pub const IP_VERSION_IPV4: u8 = 4;
pub const IP_HDR_SIZE_MIN: usize = 20;

pub const IP_HDR_FLAG_MF: u16 = 0x2000;
pub const IP_HDR_FLAG_DF: u16 = 0x4000;
pub const IP_HDR_OFFSET_MASK: u16 = 0x1fff;

pub const IP_PROTOCOL_ICMP: u8 = 1;
pub const IP_PROTOCOL_TCP: u8 = 6;
pub const IP_PROTOCOL_UDP: u8 = 17;

// --- IP 層プロトコルハンドラ登録 ---

pub type ProtocolHandler = fn(hdr: &IpHdr<'_>, data: &[u8], iface: &IpIface);

struct Protocol {
    protocol: u8,
    handler: ProtocolHandler,
}

static PROTOCOLS: Mutex<Vec<Protocol>> = Mutex::new(Vec::new());

pub fn register_protocol(protocol: u8, handler: ProtocolHandler) -> Result<(), ()> {
    let mut protocols = PROTOCOLS.lock().unwrap();
    if protocols.iter().any(|p| p.protocol == protocol) {
        log::error!("ip: protocol already registered: {}", protocol);
        return Err(());
    }
    protocols.push(Protocol { protocol, handler });
    log::info!("ip: protocol registered: {}", protocol);
    Ok(())
}

// --- IpAddr ---

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IpAddr(pub [u8; IP_ADDR_LEN]);

impl IpAddr {
    pub const ANY: Self = Self([0; IP_ADDR_LEN]);
    pub const BROADCAST: Self = Self([0xff; IP_ADDR_LEN]);
}

impl fmt::Display for IpAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}.{}", self.0[0], self.0[1], self.0[2], self.0[3])
    }
}

impl FromStr for IpAddr {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut octets = [0u8; IP_ADDR_LEN];
        let mut parts = s.split('.');
        for octet in &mut octets {
            *octet = parts.next().ok_or(())?.parse().map_err(|_| ())?;
        }
        if parts.next().is_some() {
            return Err(());
        }
        Ok(IpAddr(octets))
    }
}

// --- IpEndp (addr:port) ---

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IpEndp {
    pub addr: IpAddr,
    pub port: u16,
}

impl IpEndp {
    pub fn new(addr: IpAddr, port: u16) -> Self {
        Self { addr, port }
    }
}

impl fmt::Display for IpEndp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.addr, self.port)
    }
}

// --- IpHdr (ゼロコピーパーサ) ---

pub struct IpHdr<'a> {
    data: &'a [u8],
}

impl<'a> IpHdr<'a> {
    pub fn new(data: &'a [u8]) -> Option<Self> {
        if data.len() < IP_HDR_SIZE_MIN {
            return None;
        }
        Some(Self { data })
    }

    pub fn vhl(&self) -> u8 {
        self.data[0]
    }
    pub fn version(&self) -> u8 {
        self.data[0] >> 4
    }
    pub fn ihl(&self) -> u8 {
        self.data[0] & 0x0f
    }
    pub fn hlen(&self) -> usize {
        (self.ihl() as usize) * 4
    }
    pub fn tos(&self) -> u8 {
        self.data[1]
    }

    pub fn total(&self) -> u16 {
        u16::from_be_bytes([self.data[2], self.data[3]])
    }
    pub fn id(&self) -> u16 {
        u16::from_be_bytes([self.data[4], self.data[5]])
    }
    pub fn offset(&self) -> u16 {
        u16::from_be_bytes([self.data[6], self.data[7]])
    }
    pub fn ttl(&self) -> u8 {
        self.data[8]
    }
    pub fn protocol(&self) -> u8 {
        self.data[9]
    }
    pub fn sum(&self) -> u16 {
        u16::from_be_bytes([self.data[10], self.data[11]])
    }
    pub fn src(&self) -> IpAddr {
        IpAddr([self.data[12], self.data[13], self.data[14], self.data[15]])
    }
    pub fn dst(&self) -> IpAddr {
        IpAddr([self.data[16], self.data[17], self.data[18], self.data[19]])
    }
    pub fn as_bytes(&self) -> &[u8] {
        &self.data[..self.hlen().min(self.data.len())]
    }
}

// --- IpIface (論理インタフェース) ---

pub struct IpIface {
    dev: Arc<Device>,
    unicast: IpAddr,
    netmask: IpAddr,
    broadcast: IpAddr,
}

impl IpIface {
    pub fn dev(&self) -> &Arc<Device> {
        &self.dev
    }
    pub fn unicast(&self) -> IpAddr {
        self.unicast
    }
    pub fn netmask(&self) -> IpAddr {
        self.netmask
    }
    pub fn broadcast(&self) -> IpAddr {
        self.broadcast
    }

    /// addr が同じサブネットに属するか判定する。
    pub fn contains(&self, addr: IpAddr) -> bool {
        (0..IP_ADDR_LEN)
            .all(|i| (addr.0[i] & self.netmask.0[i]) == (self.unicast.0[i] & self.netmask.0[i]))
    }
}

impl NetIface for IpIface {
    fn family(&self) -> u16 {
        FAMILY_IP
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

static IFACES: Mutex<Vec<Arc<IpIface>>> = Mutex::new(Vec::new());

/// IP インタフェースをデバイスに登録し、ルーティングテーブルにも追加する。
pub fn iface_register(dev: &Arc<Device>, unicast: &str, netmask: &str) -> Result<Arc<IpIface>, ()> {
    let unicast: IpAddr = unicast.parse().map_err(|_| {
        log::error!("ip: invalid unicast address");
    })?;
    let netmask: IpAddr = netmask.parse().map_err(|_| {
        log::error!("ip: invalid netmask");
    })?;
    let broadcast = IpAddr([
        unicast.0[0] | !netmask.0[0],
        unicast.0[1] | !netmask.0[1],
        unicast.0[2] | !netmask.0[2],
        unicast.0[3] | !netmask.0[3],
    ]);
    let iface = Arc::new(IpIface {
        dev: Arc::clone(dev),
        unicast,
        netmask,
        broadcast,
    });
    dev.add_iface(iface.clone())?;
    IFACES.lock().unwrap().push(iface.clone());

    // サブネットルートを自動追加
    let network = IpAddr([
        unicast.0[0] & netmask.0[0],
        unicast.0[1] & netmask.0[1],
        unicast.0[2] & netmask.0[2],
        unicast.0[3] & netmask.0[3],
    ]);
    route_add(network, netmask, IpAddr::ANY, iface.clone())?;

    log::info!(
        "ip: iface registered: dev={}, unicast={}, netmask={}, broadcast={}",
        dev.name,
        unicast,
        netmask,
        broadcast
    );
    Ok(iface)
}

pub fn iface_select(addr: IpAddr) -> Option<Arc<IpIface>> {
    IFACES
        .lock()
        .unwrap()
        .iter()
        .find(|i| i.unicast == addr)
        .cloned()
}

// --- ルーティング ---

pub struct IpRoute {
    pub network: IpAddr,
    pub netmask: IpAddr,
    pub nexthop: IpAddr,
    pub iface: Arc<IpIface>,
}

static ROUTES: Mutex<Vec<Arc<IpRoute>>> = Mutex::new(Vec::new());

pub fn route_add(
    network: IpAddr,
    netmask: IpAddr,
    nexthop: IpAddr,
    iface: Arc<IpIface>,
) -> Result<(), ()> {
    log::info!(
        "ip: route added: network={}, netmask={}, nexthop={}, dev={}",
        network,
        netmask,
        nexthop,
        iface.dev.name
    );
    ROUTES.lock().unwrap().push(Arc::new(IpRoute {
        network,
        netmask,
        nexthop,
        iface,
    }));
    Ok(())
}

/// 最長プレフィクスマッチでルートを検索する。
pub fn route_lookup(dst: IpAddr) -> Option<Arc<IpRoute>> {
    let routes = ROUTES.lock().unwrap();
    let mut best: Option<Arc<IpRoute>> = None;
    let mut best_prefix: i32 = -1;
    for route in routes.iter() {
        let matches =
            (0..IP_ADDR_LEN).all(|i| (dst.0[i] & route.netmask.0[i]) == route.network.0[i]);
        if matches {
            let prefix: i32 = route.netmask.0.iter().map(|b| b.count_ones() as i32).sum();
            if prefix > best_prefix {
                best = Some(route.clone());
                best_prefix = prefix;
            }
        }
    }
    best
}

pub fn set_default_gateway(iface: &Arc<IpIface>, gw: IpAddr) -> Result<(), ()> {
    route_add(IpAddr::ANY, IpAddr::ANY, gw, iface.clone())
}

// --- IP パケット組み立て ---

fn build_packet(
    protocol: u8,
    data: &[u8],
    src: IpAddr,
    dst: IpAddr,
    id: u16,
) -> Result<Vec<u8>, ()> {
    let total = IP_HDR_SIZE_MIN + data.len();
    if total > u16::MAX as usize {
        log::error!("ip: packet too long: total={}", total);
        return Err(());
    }
    let mut buf = vec![0u8; total];
    buf[0] = (IP_VERSION_IPV4 << 4) | ((IP_HDR_SIZE_MIN / 4) as u8);
    buf[2..4].copy_from_slice(&(total as u16).to_be_bytes());
    buf[4..6].copy_from_slice(&id.to_be_bytes());
    buf[8] = 255; // TTL
    buf[9] = protocol;
    buf[12..16].copy_from_slice(&src.0);
    buf[16..20].copy_from_slice(&dst.0);
    let checksum = util::cksum16(&buf[..IP_HDR_SIZE_MIN], 0);
    buf[10..12].copy_from_slice(&checksum.to_ne_bytes());
    buf[IP_HDR_SIZE_MIN..].copy_from_slice(data);
    Ok(buf)
}

// --- IP 出力 ---

fn output_device(iface: &IpIface, buf: &[u8], target: IpAddr) -> Result<(), ()> {
    log::debug!(
        "ip: output_device: dev={}, len={}, target={}",
        iface.dev.name,
        buf.len(),
        target
    );
    let hwaddr = [0u8; ADDR_LEN]; // Phase 3 (ARP) で解決する
                                  // FLAG_NEED_ARP の場合は本来 ARP 解決が必要だが、
                                  // Loopback では不要なのでそのまま送信する。
    let _ = target;
    iface.dev.output(net::PROTOCOL_TYPE_IP, buf, &hwaddr)
}

pub fn output(protocol: u8, data: &[u8], src: IpAddr, dst: IpAddr) -> Result<(), ()> {
    log::debug!(
        "ip: output: {} => {}, protocol={}, len={}",
        src,
        dst,
        protocol,
        data.len()
    );

    if src == IpAddr::ANY && dst == IpAddr::BROADCAST {
        log::error!("ip: source address is required for broadcast");
        return Err(());
    }

    let route = match route_lookup(dst) {
        Some(r) => r,
        None => {
            log::error!("ip: no route to host: dst={}", dst);
            return Err(());
        }
    };
    let iface = &route.iface;
    let nexthop = if route.nexthop != IpAddr::ANY {
        route.nexthop
    } else {
        dst
    };

    let src = if src == IpAddr::ANY {
        iface.unicast
    } else {
        src
    };

    if (iface.dev.mtu as usize) < IP_HDR_SIZE_MIN + data.len() {
        log::error!(
            "ip: too long for MTU: dev={}, mtu={}",
            iface.dev.name,
            iface.dev.mtu
        );
        return Err(());
    }

    let id = rand_u16();
    let buf = build_packet(protocol, data, src, dst, id)?;
    output_device(iface, &buf, nexthop)
}

fn rand_u16() -> u16 {
    // 簡易的な乱数。Phase 4 以降で rand クレート等に置換可能。
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    use std::time::SystemTime;
    let mut hasher = DefaultHasher::new();
    SystemTime::now().hash(&mut hasher);
    std::thread::current().id().hash(&mut hasher);
    hasher.finish() as u16
}

// --- IP 入力 ---

fn input(data: &[u8], dev: &Device) {
    log::debug!("ip: input: dev={}, len={}", dev.name, data.len());

    let hdr = match IpHdr::new(data) {
        Some(h) => h,
        None => {
            log::error!("ip: too short: len={}", data.len());
            return;
        }
    };
    if hdr.version() != IP_VERSION_IPV4 {
        log::error!("ip: not IPv4: version={}", hdr.version());
        return;
    }
    let hlen = hdr.hlen();
    if data.len() < hlen {
        log::error!("ip: header truncated: len={} < hlen={}", data.len(), hlen);
        return;
    }
    let total = hdr.total() as usize;
    if data.len() < total {
        log::error!("ip: total truncated: len={} < total={}", data.len(), total);
        return;
    }
    if util::cksum16(&data[..hlen], 0) != 0 {
        log::error!("ip: checksum error: sum=0x{:04x}", hdr.sum());
        return;
    }
    let offset = hdr.offset();
    if offset & IP_HDR_FLAG_MF != 0 || offset & IP_HDR_OFFSET_MASK != 0 {
        log::error!("ip: fragments not supported");
        return;
    }

    // デバイスに紐付いた IpIface を取得
    let net_iface = match dev.get_iface(FAMILY_IP) {
        Some(i) => i,
        None => return,
    };
    let iface = match net_iface.as_any().downcast_ref::<IpIface>() {
        Some(i) => i,
        None => return,
    };

    // 宛先フィルタリング
    if hdr.dst() != iface.unicast && hdr.dst() != iface.broadcast && hdr.dst() != IpAddr::BROADCAST
    {
        return;
    }

    log::debug!(
        "ip: accepted: {} => {}, protocol={}, len={}",
        hdr.src(),
        hdr.dst(),
        hdr.protocol(),
        total
    );

    // 上位プロトコルにディスパッチ
    let handler = {
        let protocols = PROTOCOLS.lock().unwrap();
        protocols
            .iter()
            .find(|p| p.protocol == hdr.protocol())
            .map(|p| p.handler)
    };
    if let Some(handler) = handler {
        handler(&hdr, &data[hlen..total], iface);
    } else {
        // 未知プロトコル → ICMP Destination Unreachable
        let icmp_data_len = hlen + std::cmp::min(8, total - hlen);
        let _ = crate::icmp::output(
            crate::icmp::ICMP_TYPE_DEST_UNREACH,
            crate::icmp::ICMP_CODE_PROTO_UNREACH,
            0,
            &data[..icmp_data_len],
            iface.unicast,
            hdr.src(),
        );
    }
}

pub fn init() -> Result<(), ()> {
    net::register_protocol(net::PROTOCOL_TYPE_IP, input)?;
    Ok(())
}
