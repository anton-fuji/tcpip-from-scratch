//! Loopback デバイスを登録し、ダミープロトコルを登録して
//! パケットの送信→受信→ディスパッチが動くことを確認する

use my_tcpip::{device, driver, net};

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("debug")).init();

    net::init().expect("init failed");

    let loopback = driver::loopback::init();
    log::info!(
        "loopback device: name={}, index={}, mtu={}",
        loopback.name,
        loopback.index,
        loopback.mtu
    );

    const TEST_PROTOCOL: u16 = 0x9999;
    net::register_protocol(TEST_PROTOCOL, test_handler).expect("register failed");

    net::run().expect("run failed");

    let payload = b"Hello, TCP/IP stack!";
    let empty_dst = [0u8; device::ADDR_LEN];
    loopback
        .output(TEST_PROTOCOL, payload, &empty_dst)
        .expect("output failed");

    assert!(net::register_protocol(TEST_PROTOCOL, test_handler).is_err());

    net::shutdown();

    log::info!("Phase 1 complete!");
}

fn test_handler(data: &[u8], dev: &device::Device) {
    let msg = std::str::from_utf8(data).unwrap_or("<non-utf8>");
    log::info!(
        "==> test_handler called: dev={}, len={}, data=\"{}\"",
        dev.name,
        data.len(),
        msg
    );
}
