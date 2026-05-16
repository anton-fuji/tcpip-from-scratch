//! Phase 1 動作確認用バイナリ。
//!
//! Loopback デバイスを登録し、ダミープロトコルを登録して
//! パケットの送信→受信→ディスパッチが動くことを確認する。

use my_tcpip::{device, driver, net};

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("debug")).init();

    // step 0: スタック初期化
    net::init().expect("init failed");

    // step 2: Loopback デバイスを登録
    let loopback = driver::loopback::init();
    log::info!(
        "loopback device: name={}, index={}, mtu={}",
        loopback.name,
        loopback.index,
        loopback.mtu
    );

    // step 3: テスト用プロトコルを登録
    const TEST_PROTOCOL: u16 = 0x9999;
    net::register_protocol(TEST_PROTOCOL, test_handler).expect("register failed");

    // step 0: デバイス起動
    net::run().expect("run failed");

    // Loopback 経由でパケットを送信 → 受信 → ハンドラ呼び出しを確認
    let payload = b"Hello, TCP/IP stack!";
    let empty_dst = [0u8; device::ADDR_LEN];
    loopback
        .output(TEST_PROTOCOL, payload, &empty_dst)
        .expect("output failed");

    // 二重登録がエラーになることを確認
    assert!(net::register_protocol(TEST_PROTOCOL, test_handler).is_err());

    // step 0: シャットダウン
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
