//! ネットワークスタックのライフサイクル管理とプロトコルディスパッチ。

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use crate::device::Device;

pub const PROTOCOL_TYPE_IP: u16 = 0x0800;
pub const PROTOCOL_TYPE_ARP: u16 = 0x0806;
pub const PROTOCOL_TYPE_IPV6: u16 = 0x86dd;

pub type ProtocolHandler = fn(data: &[u8], dev: &Device);

struct Protocol {
    ty: u16,
    handler: ProtocolHandler,
}

struct InputEntry {
    ty: u16,
    data: Vec<u8>,
    dev: Arc<Device>,
}

static PROTOCOLS: Mutex<Vec<Protocol>> = Mutex::new(Vec::new());
static INPUT_QUEUE: Mutex<VecDeque<InputEntry>> = Mutex::new(VecDeque::new());

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

pub fn input_handler(ty: u16, data: &[u8], dev: &Device) -> Result<(), ()> {
    log::debug!(
        "input: dev={}, type=0x{:04x}, len={}",
        dev.name, ty, data.len()
    );
    log::trace!("\n{}", crate::util::HexDump(data));

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

    softirq_handler();
    Ok(())
}

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

pub fn init() -> Result<(), ()> {
    log::info!("initializing network stack...");
    crate::ip::init()?;
    crate::icmp::init()?;
    log::info!("network stack initialized");
    Ok(())
}

pub fn run() -> Result<(), ()> {
    log::info!("starting network stack...");
    crate::device::try_foreach(|dev| dev.open())?;
    log::info!("network stack started");
    Ok(())
}

pub fn shutdown() {
    log::info!("shutting down network stack...");
    crate::device::foreach(|dev| {
        let _ = dev.close();
    });
    log::info!("network stack shut down");
}
