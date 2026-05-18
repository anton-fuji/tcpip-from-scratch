//! ネットワークデバイスの抽象化
//!
//! - `Device`: 1つのネットワークインターフェースを表す
//! - `Ops`: ドライバが実装するトレイト (open / close / transmit)
//! - `register()`: グローバルなデバイスリストに登録

use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, Mutex};

/// ハードウェアアドレスの最大長
pub const ADDR_LEN: usize = 16;

// デバイスタイプ
pub const TYPE_DUMMY: u16 = 0x0000;
pub const TYPE_LOOPBACK: u16 = 0x0001;
pub const TYPE_ETHERNET: u16 = 0x0002;

// デバイスフラグ
pub const FLAG_UP: u16 = 0x0001;
pub const FLAG_LOOPBACK: u16 = 0x0010;
pub const FLAG_BROADCAST: u16 = 0x0020;
pub const FLAG_P2P: u16 = 0x0040;
pub const FLAG_NEED_ARP: u16 = 0x0100;

/// デバイスドライバが実装するトレイト
/// open/close はデフォルト実装で何もしない
pub trait Ops: Send + Sync {
    fn open(&self, _dev: &Device) -> Result<(), ()> {
        Ok(())
    }

    fn close(&self, _dev: &Device) -> Result<(), ()> {
        Ok(())
    }

    /// フレームを送信する
    /// `ty`: プロトコル種別 (EtherType)
    /// `data`: ペイロード
    /// `dst`: 宛先ハードウェアアドレス
    fn transmit(&self, dev: &Device, ty: u16, data: &[u8], dst: &[u8]) -> Result<(), ()>;
}

/// ネットワークデバイス
/// 1つの物理/仮想インターフェースに対応する
pub struct Device {
    pub index: usize,
    pub name: String,
    pub ty: u16,
    pub mtu: u16,
    pub hlen: u16, // ヘッダ長
    pub alen: u16, // アドレス長
    pub addr: Mutex<[u8; ADDR_LEN]>,
    pub peer: [u8; ADDR_LEN],
    pub broadcast: [u8; ADDR_LEN],
    flags: AtomicU16,
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
            ops,
        }
    }

    pub fn flags(&self) -> u16 {
        self.flags.load(Ordering::Acquire)
    }

    pub fn is_up(&self) -> bool {
        self.flags() & FLAG_UP != 0
    }

    fn state_str(&self) -> &'static str {
        if self.is_up() {
            "UP"
        } else {
            "DOWN"
        }
    }

    /// デバイスを起動する
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

    /// デバイスを停止する
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

    /// フレームを送信する。MTUチェック付き
    pub fn output(&self, ty: u16, data: &[u8], dst: &[u8]) -> Result<(), ()> {
        if !self.is_up() {
            log::error!("not opened: dev={}", self.name);
            return Err(());
        }
        if data.len() > self.mtu as usize {
            log::error!(
                "too long: dev={}, mtu={}, len={}",
                self.name,
                self.mtu,
                data.len()
            );
            return Err(());
        }
        log::debug!(
            "output: dev={}, type=0x{:04x}, len={}",
            self.name,
            ty,
            data.len()
        );
        log::trace!("\n{}", crate::util::HexDump(data));
        self.ops.transmit(self, ty, data, dst).map_err(|_| {
            log::error!("transmit failed: dev={}", self.name);
        })
    }
}

/// グローバルなデバイスリスト
static DEVICES: Mutex<Vec<Arc<Device>>> = Mutex::new(Vec::new());

/// デバイスを登録し、index と name を自動付与する
pub fn register(mut dev: Device) -> Arc<Device> {
    let mut devices = DEVICES.lock().unwrap();
    dev.index = devices.len();
    dev.name = format!("net{}", dev.index);
    log::info!("registered: dev={}, type=0x{:04x}", dev.name, dev.ty);
    let arc = Arc::new(dev);
    devices.push(arc.clone());
    arc
}

/// index でデバイスを取得する
pub fn by_index(index: usize) -> Option<Arc<Device>> {
    DEVICES.lock().unwrap().get(index).cloned()
}

/// 全デバイスに対して関数を実行する
pub fn foreach<F: FnMut(&Device)>(mut f: F) {
    for dev in DEVICES.lock().unwrap().iter() {
        f(dev);
    }
}

/// 全デバイスに対して関数を実行し、エラーがあれば即座に返す
pub fn try_foreach<F: FnMut(&Device) -> Result<(), ()>>(mut f: F) -> Result<(), ()> {
    for dev in DEVICES.lock().unwrap().iter() {
        f(dev)?;
    }
    Ok(())
}
