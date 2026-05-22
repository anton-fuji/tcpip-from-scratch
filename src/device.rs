//! ネットワークデバイスの抽象化。
//!
//! Phase 2 で追加: NetIface トレイト、ifaces フィールド

use std::any::Any;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, Mutex};

pub const ADDR_LEN: usize = 16;

pub const TYPE_DUMMY: u16 = 0x0000;
pub const TYPE_LOOPBACK: u16 = 0x0001;
pub const TYPE_ETHERNET: u16 = 0x0002;

pub const FLAG_UP: u16 = 0x0001;
pub const FLAG_LOOPBACK: u16 = 0x0010;
pub const FLAG_BROADCAST: u16 = 0x0020;
pub const FLAG_P2P: u16 = 0x0040;
pub const FLAG_NEED_ARP: u16 = 0x0100;

pub const FAMILY_IP: u16 = 1;
pub const FAMILY_IPV6: u16 = 2;

pub trait Ops: Send + Sync {
    fn open(&self, _dev: &Device) -> Result<(), ()> {
        Ok(())
    }
    fn close(&self, _dev: &Device) -> Result<(), ()> {
        Ok(())
    }
    fn transmit(&self, dev: &Device, ty: u16, data: &[u8], dst: &[u8]) -> Result<(), ()>;
}

/// 論理インタフェース (IP, IPv6 など) のトレイト。
/// Device に紐付けて、プロトコル層の情報を持たせる。
pub trait NetIface: Send + Sync + 'static {
    fn family(&self) -> u16;
    fn as_any(&self) -> &dyn Any;
}

pub struct Device {
    pub index: usize,
    pub name: String,
    pub ty: u16,
    pub mtu: u16,
    pub hlen: u16,
    pub alen: u16,
    pub addr: Mutex<[u8; ADDR_LEN]>,
    pub peer: [u8; ADDR_LEN],
    pub broadcast: [u8; ADDR_LEN],
    flags: AtomicU16,
    ifaces: Mutex<Vec<Arc<dyn NetIface>>>,
    ops: Box<dyn Ops>,
}

impl Device {
    pub fn new(ty: u16, mtu: u16, flags: u16, ops: Box<dyn Ops>) -> Self {
        Self {
            index: 0,
            name: String::new(),
            ty,
            mtu,
            hlen: 0,
            alen: 0,
            addr: Mutex::new([0; ADDR_LEN]),
            peer: [0; ADDR_LEN],
            broadcast: [0; ADDR_LEN],
            flags: AtomicU16::new(flags),
            ifaces: Mutex::new(Vec::new()),
            ops,
        }
    }

    /// 論理インタフェースを追加する。同じ family の二重登録はエラー。
    pub fn add_iface(&self, iface: Arc<dyn NetIface>) -> Result<(), ()> {
        let mut ifaces = self.ifaces.lock().unwrap();
        let family = iface.family();
        if ifaces.iter().any(|existing| existing.family() == family) {
            log::error!("iface already exists: dev={}, family={}", self.name, family);
            return Err(());
        }
        ifaces.push(iface);
        Ok(())
    }

    /// family を指定して論理インタフェースを取得する。
    pub fn get_iface(&self, family: u16) -> Option<Arc<dyn NetIface>> {
        self.ifaces
            .lock()
            .unwrap()
            .iter()
            .find(|iface| iface.family() == family)
            .cloned()
    }

    pub fn flags(&self) -> u16 {
        self.flags.load(Ordering::Acquire)
    }

    pub fn is_up(&self) -> bool {
        self.flags() & FLAG_UP != 0
    }

    fn state_str(&self) -> &'static str {
        if self.is_up() { "UP" } else { "DOWN" }
    }

    pub fn open(&self) -> Result<(), ()> {
        if self.is_up() {
            log::error!("already opened: dev={}", self.name);
            return Err(());
        }
        self.ops.open(self).map_err(|_| {
            log::error!("open failed: dev={}", self.name);
        })?;
        self.flags.fetch_or(FLAG_UP, Ordering::AcqRel);
        log::info!("dev={}, state={}", self.name, self.state_str());
        Ok(())
    }

    pub fn close(&self) -> Result<(), ()> {
        if !self.is_up() {
            log::error!("not opened: dev={}", self.name);
            return Err(());
        }
        self.ops.close(self).map_err(|_| {
            log::error!("close failed: dev={}", self.name);
        })?;
        self.flags.fetch_and(!FLAG_UP, Ordering::AcqRel);
        log::info!("dev={}, state={}", self.name, self.state_str());
        Ok(())
    }

    pub fn output(&self, ty: u16, data: &[u8], dst: &[u8]) -> Result<(), ()> {
        if !self.is_up() {
            log::error!("not opened: dev={}", self.name);
            return Err(());
        }
        if data.len() > self.mtu as usize {
            log::error!(
                "too long: dev={}, mtu={}, len={}",
                self.name, self.mtu, data.len()
            );
            return Err(());
        }
        log::debug!("output: dev={}, type=0x{:04x}, len={}", self.name, ty, data.len());
        log::trace!("\n{}", crate::util::HexDump(data));
        self.ops.transmit(self, ty, data, dst).map_err(|_| {
            log::error!("transmit failed: dev={}", self.name);
        })
    }
}

static DEVICES: Mutex<Vec<Arc<Device>>> = Mutex::new(Vec::new());

pub fn register(mut dev: Device) -> Arc<Device> {
    let mut devices = DEVICES.lock().unwrap();
    dev.index = devices.len();
    dev.name = format!("net{}", dev.index);
    log::info!("registered: dev={}, type=0x{:04x}", dev.name, dev.ty);
    let arc = Arc::new(dev);
    devices.push(arc.clone());
    arc
}

pub fn by_index(index: usize) -> Option<Arc<Device>> {
    DEVICES.lock().unwrap().get(index).cloned()
}

pub fn foreach<F: FnMut(&Device)>(mut f: F) {
    for dev in DEVICES.lock().unwrap().iter() {
        f(dev);
    }
}

pub fn try_foreach<F: FnMut(&Device) -> Result<(), ()>>(mut f: F) -> Result<(), ()> {
    for dev in DEVICES.lock().unwrap().iter() {
        f(dev)?;
    }
    Ok(())
}
