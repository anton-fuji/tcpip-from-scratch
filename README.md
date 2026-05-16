# TCP/IP from Scratch

Rust で作る自作 TCP/IP プロトコルスタック。  
[microps-rs](https://github.com/pandax381/microps-rs) を参考に、ステップバイステップで実装する。

## Phase 1: 基盤 (step 0-3)

### やっていること

```
main.rs (テストバイナリ)
  │
  ├── net::init()           ← step 0: スタック初期化
  ├── driver::loopback::init() ← step 2: Loopback デバイス登録
  ├── net::register_protocol() ← step 3: プロトコルハンドラ登録
  ├── net::run()            ← step 0: 全デバイス open
  │
  ├── device.output()       ← フレーム送信
  │     └── LoopbackOps::transmit()
  │           └── net::input_handler()  ← 受信キューに入れる
  │                 └── softirq_handler()
  │                       └── test_handler()  ← ディスパッチ！
  │
  └── net::shutdown()       ← step 0: 全デバイス close
```

### 各ファイルの役割

| ファイル | step | 役割 |
|---------|------|------|
| `src/lib.rs` | - | モジュール宣言 |
| `src/net.rs` | 0, 3 | ライフサイクル管理 + プロトコルディスパッチ |
| `src/device.rs` | 1 | Device 構造体、Ops トレイト、デバイス登録 |
| `src/driver/loopback.rs` | 2 | Loopback ドライバ（最もシンプルな Ops 実装） |
| `src/util.rs` | - | チェックサム (RFC 1071)、HexDump |
| `src/main.rs` | - | 動作確認用バイナリ |

### 実行方法

```sh
RUST_LOG=debug cargo run
```

### 設計判断メモ

- **`std` ベース**: microps-rs は `no_std` + `spin::Mutex` だが、学習用なので `std::sync::Mutex` を使う。後から `no_std` に移行も可能。
- **`log` クレート**: microps-rs は自前マクロだが、Rust エコシステム標準の `log` + `env_logger` を使う。
- **softirq 簡略化**: microps-rs は POSIX シグナル + スレッドで softirq を実現しているが、Phase 1 では `input_handler` 内で即座に処理する。Phase 4 (TAP ドライバ) で本格的に分離する。

## 次のステップ (Phase 2)

Phase 2 では IP 層を実装する:
- `src/ether.rs` — Ethernet ヘッダのパース (EtherAddr, EtherHdr)
- `src/ip.rs` — IP パケットの入出力、IpIface、ルーティング
- `src/icmp.rs` — ICMP Echo Reply (ping が通る！)
