# Phase 1: 基盤 — 詳細まとめ

## 概要

Phase 1 では TCP/IP プロトコルスタックの「骨格」を作る。
ネットワークプロトコルの知識はまだ不要で、ここで作るのは純粋なソフトウェアアーキテクチャだ。

やっていることは3つ:

1. **デバイスを抽象化する** — 物理NIC・TAP・Loopback を同じインターフェースで扱えるようにする
2. **プロトコルを差し込み可能にする** — IP や ARP のハンドラを後から登録できる仕組みを作る
3. **パケットの流れを作る** — 送信→受信→ディスパッチのパイプラインを通す

これが動けば、あとは各層 (Ethernet, IP, TCP, ...) を「差し込む」だけでスタックが育っていく。

---

## Step 0: ライフサイクル管理 (`net.rs`)

### 何をやっているか

スタック全体の起動・停止を管理する3つの関数を定義する。

```
net::init()     → 各層の初期化（Phase 1 では空）
net::run()      → 登録済み全デバイスを open
net::shutdown() → 登録済み全デバイスを close
```

### なぜ必要か

TCP/IP スタックは複数の層（Ethernet → IP → TCP → ...）が協調して動く。
各層は初期化時にプロトコルハンドラを登録する必要があり、その順序も重要だ。
たとえば IP は `net::register_protocol(0x0800, ip::input)` で自分のハンドラを登録するが、
これは Ethernet 層がフレームを受信したときに「EtherType が 0x0800 なら IP に渡す」と
判断するための仕組みだ。

`init()` にこの初期化を集約しておくことで、呼び出し順を一箇所で管理できる。

```rust
// Phase 2 以降、init() の中身はこう育つ:
pub fn init() -> Result<(), ()> {
    arp::init()?;   // ARP ハンドラ登録
    ip::init()?;    // IP ハンドラ登録
    icmp::init()?;  // ICMP ハンドラ登録
    udp::init()?;   // UDP ハンドラ登録
    tcp::init()?;   // TCP ハンドラ登録
    Ok(())
}
```

### コードのポイント

`run()` は `device::try_foreach(|dev| dev.open())` で全デバイスを起動する。
`try_foreach` はエラーが出た時点で即 `Err` を返す。
`shutdown()` は `foreach` を使い、エラーがあっても全デバイスの close を試みる。
「起動は1つでも失敗したら止める、停止は最後までやりきる」という設計意図がある。

---

## Step 1: デバイス抽象化 (`device.rs`)

### 何をやっているか

ネットワークデバイス（NIC、TAP、Loopback など）を統一的に扱うための仕組みを作る。

### Device 構造体

```rust
pub struct Device {
    pub index: usize,           // デバイス番号 (0, 1, 2, ...)
    pub name: String,           // "net0", "net1", ...
    pub ty: u16,                // TYPE_LOOPBACK, TYPE_ETHERNET, ...
    pub mtu: u16,               // 最大転送単位（バイト数）
    pub hlen: u16,              // リンク層ヘッダの長さ
    pub alen: u16,              // ハードウェアアドレスの長さ
    pub addr: Mutex<[u8; 16]>,  // 自分の HW アドレス (MAC等)
    pub broadcast: [u8; 16],    // ブロードキャスト HW アドレス
    flags: AtomicU16,           // FLAG_UP など
    ops: Box<dyn Ops>,          // ドライバの実装
}
```

重要なのは最後の `ops` フィールド。これがドライバの差し替えポイントになる。

### Ops トレイト — ドライバのインターフェース

```rust
pub trait Ops: Send + Sync {
    fn open(&self, dev: &Device) -> Result<(), ()>  { Ok(()) }
    fn close(&self, dev: &Device) -> Result<(), ()> { Ok(()) }
    fn transmit(&self, dev: &Device, ty: u16, data: &[u8], dst: &[u8]) -> Result<(), ()>;
}
```

`open` と `close` にはデフォルト実装がある（何もしない）。
Loopback のように特別な初期化が不要なドライバは `transmit` だけ実装すればいい。
TAP ドライバ（Phase 4）では `open` でファイルディスクリプタを開き、
`close` で閉じる処理を入れることになる。

### なぜ `Box<dyn Ops>` か

Rust のトレイトオブジェクト (`dyn Ops`) を使うことで、異なるドライバの実装を
同じ `Device` 構造体に格納できる。`Box` でヒープに置くのは、`Ops` の実装ごとに
サイズが異なるため。

```
Device { ops: Box<LoopbackOps> }  ← Loopback デバイス
Device { ops: Box<TapOps> }       ← TAP デバイス (Phase 4)
```

どちらも `Arc<Device>` として同じリストに入り、同じ `output()` メソッドで送信できる。

### フラグ管理に AtomicU16 を使う理由

`flags` は「デバイスが UP かどうか」を示すフィールドで、複数スレッドから
読み書きされる可能性がある。`Mutex` だとロック取得のコストが大きいので、
ビットフラグの set/clear には `AtomicU16` の `fetch_or` / `fetch_and` を使う。

```rust
// open 時: FLAG_UP ビットを立てる
self.flags.fetch_or(FLAG_UP, Ordering::AcqRel);

// close 時: FLAG_UP ビットを落とす
self.flags.fetch_and(!FLAG_UP, Ordering::AcqRel);
```

### グローバルデバイスリスト

```rust
static DEVICES: Mutex<Vec<Arc<Device>>> = Mutex::new(Vec::new());
```

`Arc<Device>` にする理由は、デバイスへの参照を複数箇所で持つ必要があるため。
たとえば入力キュー (`InputEntry`) にもデバイスの参照を持たせるが、
キューのライフタイムはデバイスリストとは独立している。

`register()` は `Device` を受け取り、`index` と `name` を付与してリストに追加する:

```rust
pub fn register(mut dev: Device) -> Arc<Device> {
    let mut devices = DEVICES.lock().unwrap();
    dev.index = devices.len();          // 0, 1, 2, ...
    dev.name = format!("net{}", dev.index);  // "net0", "net1", ...
    let arc = Arc::new(dev);
    devices.push(arc.clone());
    arc
}
```

---

## Step 2: Loopback ドライバ (`driver/loopback.rs`)

### 何をやっているか

送信したデータがそのまま自分に返ってくる、最もシンプルなドライバ。
実際のネットワーク通信は発生しない。

### コードの全体像

```rust
struct LoopbackOps;

impl Ops for LoopbackOps {
    fn transmit(&self, dev: &Device, ty: u16, data: &[u8], _dst: &[u8]) -> Result<(), ()> {
        // 送信データをそのまま受信として処理する
        net::input_handler(ty, data, dev)
    }
}

pub fn init() -> Arc<Device> {
    let dev = Device::new(TYPE_LOOPBACK, u16::MAX, FLAG_LOOPBACK, Box::new(LoopbackOps));
    device::register(dev)
}
```

たった20行。しかしこれが Phase 1 の動作確認に必要十分だ。

### なぜ Loopback から始めるか

TAP デバイス（Linux の仮想NIC）を使うと、ioctl やシグナルハンドリングなど
プラットフォーム固有のコードが大量に必要になる。
Loopback なら OS 依存ゼロで「送信→受信→ディスパッチ」の全パイプラインを
テストできる。

### MTU が u16::MAX (65535) の理由

Loopback は物理的な制約がないので、IP パケットの最大サイズ (65535バイト) を
そのまま MTU として設定している。
Ethernet デバイス（Phase 4）では MTU = 1500 になる。

---

## Step 3: プロトコル登録とディスパッチ (`net.rs` の後半)

### 何をやっているか

受信したフレームの「種別 (EtherType)」に応じて、適切なハンドラに振り分ける仕組みを作る。

### データの流れ

```
device.output(ty, data, dst)
  │
  ▼
Ops::transmit(dev, ty, data, dst)     ← ドライバ固有の送信処理
  │
  ▼ (Loopback の場合、ここで折り返す)
net::input_handler(ty, data, dev)     ← 受信パケットをキューに入れる
  │
  ▼
softirq_handler()                     ← キューからパケットを取り出す
  │
  ▼
PROTOCOLS から ty に一致する
ハンドラを探して呼び出す               ← ディスパッチ
```

### プロトコル登録

```rust
static PROTOCOLS: Mutex<Vec<Protocol>> = Mutex::new(Vec::new());

pub fn register_protocol(ty: u16, handler: ProtocolHandler) -> Result<(), ()> {
    let mut protocols = PROTOCOLS.lock().unwrap();
    if protocols.iter().any(|p| p.ty == ty) {
        return Err(());  // 二重登録を防ぐ
    }
    protocols.push(Protocol { ty, handler });
    Ok(())
}
```

Phase 2 以降、各層がここに自分のハンドラを登録する:

```
ty = 0x0800 (IP)  → ip::input
ty = 0x0806 (ARP) → arp::input
```

### 入力キューと softirq

```rust
static INPUT_QUEUE: Mutex<VecDeque<InputEntry>> = Mutex::new(VecDeque::new());
```

なぜ `input_handler` で直接ハンドラを呼ばず、キューを経由するのか？

実際のネットワークスタックでは、パケット受信はハードウェア割り込み（またはシグナル）
の中で起きる。割り込みハンドラの中で重い処理（TCP のステートマシンなど）を
実行すると、次の割り込みが処理できなくなる。

そこで受信パケットをキューに入れるだけにして、実際の処理は
別のコンテキスト (softirq) で行う。Linux カーネルの NET_RX_SOFTIRQ と同じ考え方だ。

Phase 1 では `input_handler` 内で即座に `softirq_handler()` を呼んでいるので
実質的にはキューを経由する意味は薄いが、Phase 4 で TAP ドライバを導入すると、
シグナルハンドラ → キュー → メインスレッドで処理、という分離が活きてくる。

### ハンドラ型が `fn` ポインタである理由

```rust
pub type ProtocolHandler = fn(data: &[u8], dev: &Device);
```

クロージャ (`Box<dyn Fn(...)>`) ではなく `fn` ポインタを使っている。
各プロトコル層のハンドラは状態を持たない純粋な関数（`ip::input`, `arp::input` 等）
なので、`fn` ポインタで十分であり、ヒープ割り当ても不要。
microps-rs が `no_std` で動くことを意識した設計でもある。

---

## ユーティリティ (`util.rs`)

### RFC 1071 インターネットチェックサム

```rust
pub fn cksum16(data: &[u8], init: u32) -> u16
```

IP, ICMP, TCP, UDP すべてで使われる共通のチェックサム関数。
アルゴリズムは「16ビット単位の1の補数和の、1の補数」。

1. データを16ビット (2バイト) ずつ足し合わせる
2. 奇数バイトが余ったら、下位バイトだけ足す
3. 上位16ビットに溢れた分を下位に折り返す（キャリーの加算）
4. 最後にビット反転

```rust
// 検証側: 正しいデータのチェックサムは 0 になる
assert_eq!(cksum16(&ip_header, 0), 0);

// 計算側: チェックサムフィールドを0にしてから計算
header[10] = 0;
header[11] = 0;
let checksum = cksum16(&header, 0);
```

`init` 引数は分割計算用。TCP/UDP の疑似ヘッダチェックサムでは、
疑似ヘッダの部分和を `init` に渡してデータ部分と合算する。

### HexDump

パケットの中身をデバッグ表示するためのフォーマッタ。
`RUST_LOG=trace` で有効になる。

```
+------+-------------------------------------------------+------------------+
| 0000 | 48 65 6c 6c 6f 2c 20 54 43 50 2f 49 50 20 73 74 | Hello, TCP/IP st |
| 0010 | 61 63 6b 21                                      | ack!             |
+------+-------------------------------------------------+------------------+
```

---

## main.rs — 動作確認の流れ

実行ログとコードの対応:

```
[INFO  net]            initializing network stack...     ← net::init()
[INFO  net]            network stack initialized
[INFO  device]         registered: dev=net0, type=0x0001 ← driver::loopback::init()
[INFO  my_tcpip]       loopback device: name=net0, ...
[INFO  net]            protocol registered: type=0x9999  ← net::register_protocol()
[INFO  net]            starting network stack...         ← net::run()
[INFO  device]         dev=net0, state=UP                  ← device.open()
[INFO  net]            network stack started
[DEBUG device]         output: dev=net0, type=0x9999, ...← loopback.output()
[DEBUG loopback]       loopback: dev=net0, ...             ← LoopbackOps::transmit()
[DEBUG net]            input: dev=net0, ...                ← net::input_handler()
[INFO  my_tcpip]       ==> test_handler called: ...        ← ディスパッチ成功！
[ERROR net]            protocol already registered: ...  ← 二重登録エラー確認
[INFO  net]            shutting down network stack...    ← net::shutdown()
[INFO  device]         dev=net0, state=DOWN                ← device.close()
[INFO  net]            network stack shut down
[INFO  my_tcpip]       Phase 1 complete!
```

---

## microps-rs との設計比較

| 観点 | microps-rs | このプロジェクト |
|------|-----------|---------------|
| `std` / `no_std` | `no_std` + `alloc` | `std` |
| Mutex | `spin::Mutex` (スピンロック) | `std::sync::Mutex` |
| ログ | 自前マクロ (`infof!` 等) | `log` + `env_logger` |
| softirq | POSIX シグナル + 専用スレッド | `input_handler` 内で即実行 |
| 乱数 | `libc::rand()` | (Phase 2 で `rand` クレートを使う予定) |
| NetIface | `dyn Any` でダウンキャスト | (Phase 2 で実装) |

`no_std` を選んでいるのは、将来的に OS なしの環境 (ベアメタル) でも
動かせるようにするため。学習用途では `std` で十分であり、
`Mutex` の使い方や `Arc` の意味は同じなので、概念の理解には影響しない。

---

## Phase 2 に向けて

Phase 1 で作ったものは「フレームを送受信する基盤」だった。
Phase 2 では、その上に実際のネットワークプロトコルを載せる:

- **Ethernet ヘッダ** (`ether.rs`) — MACアドレスの解析
- **IP 入出力** (`ip.rs`) — IPアドレス、ヘッダ解析、チェックサム検証、ルーティング
- **ICMP** (`icmp.rs`) — Echo Request に Echo Reply を返す

Phase 2 が完了すると、TAP デバイス経由で `ping` が通るようになる。
これが最初の「本物のネットワーク通信が動いた」瞬間になる。
