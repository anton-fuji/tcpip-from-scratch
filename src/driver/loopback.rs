//! Loopback デバイスドライバ。

use std::sync::Arc;

use crate::device::{self, Device, Ops, FLAG_LOOPBACK, TYPE_LOOPBACK};
use crate::net;

const LOOPBACK_MTU: u16 = u16::MAX;

struct LoopbackOps;

impl Ops for LoopbackOps {
    fn transmit(&self, dev: &Device, ty: u16, data: &[u8], _dst: &[u8]) -> Result<(), ()> {
        log::debug!(
            "loopback: dev={}, type=0x{:04x}, len={}",
            dev.name,
            ty,
            data.len()
        );
        net::input_handler(ty, data, dev)
    }
}

pub fn init() -> Arc<Device> {
    let dev = Device::new(
        TYPE_LOOPBACK,
        LOOPBACK_MTU,
        FLAG_LOOPBACK,
        Box::new(LoopbackOps),
    );
    device::register(dev)
}
