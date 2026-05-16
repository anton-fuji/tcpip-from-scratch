//! ネットワークスタックのライフサイクル管理とプロトコルディスパッチ。
//!
//! - `init()` で各層を初期化
//! - `run()` でデバイスを起動
//! - `shutdown()` で停止
//! - `register_protocol()` / `input_handler()` でフレーム種別ごとの振り分け

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use crate::device::Device;

/// Ethernet フレームのプロトコル種別 (EtherType)
pub const PROTOCOL_TYPE_IP: u16 = 0x0800;
pub const PROTOCOL_TYPE_ARP: u16 = 0x0806;
pub const PROTOCOL_TYPE_IPV6: u16 = 0x86dd;

/// プロトコルハンドラの型。受信データとデバイスを受け取る。
pub type ProtocolHandler = fn(data: &[u8], dev: &Device);

struct Protocol {
    ty: u16,
    handler: ProtocolHandler,
}

/// 入力キューのエントリ。softirq で遅延処理する。
struct InputEntry {
    ty: u16,
    data: Vec<u8>,
    dev: Arc<Device>,
}

/// 登録済みプロトコル一覧
static PROTOCOLS: Mutex<Vec<Protocol>> = Mutex::new(Vec::new());

/// 受信パケットのキュー
static INPUT_QUEUE: Mutex<VecDeque<InputEntry>> = Mutex::new(VecDeque::new());

/// プロトコルハンドラを登録する。
/// 同じ type を二重登録するとエラー。
pub fn register_protocol(ty: u16, handler: ProtocolHandler) -> Result<(), ()> {
    let mut protocols = PROTOCOLS.lock().unwrap();
    if protocols.iter().any(|p| p.ty == ty) {
        log::error!("protocol already registered: type=0x{ty:04x}");
        return Err(());
    }
    protocols.push(Protocol { ty, handler });
    log::info!("protocol registered: type=0x{ty:04x}");
    Ok(())
}

/// デバイスドライバから呼ばれる入力ハンドラ。
/// 受信フレームをキューに入れ、softirq で処理する。
pub fn input_handler(ty: u16, data: &[u8], dev: &Device) -> Result<(), ()> {
    log::debug!(
        "input: dev={}, type=0x{:04x}, len={}",
        dev.name,
        ty,
        data.len()
    );
    log::trace!("\n{}", crate::util::HexDump(data));

    // 登録されていないプロトコルは無視
    if !PROTOCOLS.lock().unwrap().iter().any(|p| p.ty == ty) {
        return Ok(());
    }

    let arc = match crate::device::by_index(dev.index) {
        Some(a) => a,
        None => {
            log::error!("device not registered: index={}", dev.index);
            return Err(());
        }
    };

    INPUT_QUEUE.lock().unwrap().push_back(InputEntry {
        ty,
        data: data.to_vec(),
        dev: arc,
    });

    // 即座に処理（後のフェーズで softirq に置き換え可能）
    softirq_handler();
    Ok(())
}

/// キューに溜まったパケットを順に処理する。
pub fn softirq_handler() {
    loop {
        let entry = match INPUT_QUEUE.lock().unwrap().pop_front() {
            Some(e) => e,
            None => break,
        };
        let handler = {
            let protocols = PROTOCOLS.lock().unwrap();
            protocols.iter().find(|p| p.ty == entry.ty).map(|p| p.handler)
        };
        if let Some(handler) = handler {
            handler(&entry.data, &entry.dev);
        }
    }
}

/// スタック全体を初期化する。
/// 後のフェーズで ARP, IP, ICMP, UDP, TCP の init を呼ぶ。
pub fn init() -> Result<(), ()> {
    log::info!("initializing network stack...");
    // Phase 2 以降: arp::init(), ip::init(), icmp::init(), ...
    log::info!("network stack initialized");
    Ok(())
}

/// デバイスを起動する。
pub fn run() -> Result<(), ()> {
    log::info!("starting network stack...");
    crate::device::try_foreach(|dev| dev.open())?;
    log::info!("network stack started");
    Ok(())
}

/// スタックを停止する。
pub fn shutdown() {
    log::info!("shutting down network stack...");
    crate::device::foreach(|dev| {
        let _ = dev.close();
    });
    log::info!("network stack shut down");
}
