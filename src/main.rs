//! Phase 2 動作確認用バイナリ
//!
//! Loopback デバイスに IP インタフェースを登録し、
//! ICMP Echo Request を送って Echo Reply が返ることを確認する。

use my_tcpip::{driver, ip, net};

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("debug")).init();

    // スタック初期化 (IP + ICMP ハンドラが登録される)
    net::init().expect("init failed");

    // Loopback デバイスを登録
    let loopback = driver::loopback::init();

    // IP インタフェースを登録 (127.0.0.1/8)
    let iface = ip::iface_register(&loopback, "127.0.0.1", "255.0.0.0")
        .expect("iface_register failed");

    log::info!(
        "loopback: unicast={}, netmask={}, broadcast={}",
        iface.unicast(), iface.netmask(), iface.broadcast()
    );

    // デバイス起動
    net::run().expect("run failed");

    // ICMP Echo Request を手動で組み立てて送信
    // → IP層 → Loopback → IP入力 → ICMP入力 → Echo Reply 送信
    let echo_id: u16 = 0x1234;
    let echo_seq: u16 = 1;
    let payload = b"ping!hello";

    let mut icmp_buf = vec![0u8; 8 + payload.len()];
    icmp_buf[0] = 8; // type: Echo Request
    icmp_buf[1] = 0; // code
    icmp_buf[4..6].copy_from_slice(&echo_id.to_be_bytes());
    icmp_buf[6..8].copy_from_slice(&echo_seq.to_be_bytes());
    icmp_buf[8..].copy_from_slice(payload);
    let sum = my_tcpip::util::cksum16(&icmp_buf, 0);
    icmp_buf[2..4].copy_from_slice(&sum.to_ne_bytes());

    log::info!("--- Sending ICMP Echo Request ---");
    ip::output(
        ip::IP_PROTOCOL_ICMP,
        &icmp_buf,
        ip::IpAddr([127, 0, 0, 1]),
        ip::IpAddr([127, 0, 0, 1]),
    )
    .expect("ip::output failed");

    log::info!("--- Done! Echo Reply should appear above ---");

    net::shutdown();
}
