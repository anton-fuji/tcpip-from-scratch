# Phase 2: IP + ICMP — 詳細解説

## 概要

Phase 1 では「フレームを送受信する基盤」を作った。
Phase 2 では、その基盤の上に実際のネットワークプロトコルを載せる。

追加したもの:

1. **Ethernet ヘッダ定義** (`ether.rs`) — MAC アドレスとフレームヘッダの表現
2. **IP 層** (`ip.rs`) — アドレス、ヘッダ解析、論理インタフェース、ルーティング、パケット入出力
3. **ICMP** (`icmp.rs`) — Echo Request/Reply と Destination Unreachable

変更したもの:

4. **Device** (`device.rs`) — `NetIface` トレイトの追加
5. **net.rs** — `init()` で IP, ICMP を初期化
6. **main.rs** — ICMP Echo Request を手動で組み立ててテスト

Phase 2 が終わると、Loopback 経由で ping (Echo Request → Echo Reply) が通る。

---

## 動作確認ログの読み方

```
[INFO ] --- Sending ICMP Echo Request ---
[DEBUG] ip: output: 127.0.0.1 => 127.0.0.1, protocol=1, len=18    ← (1) IP出力
[DEBUG] ip: output_device: dev=net0, len=38, target=127.0.0.1      ← (2) IPヘッダ付きで38B
[DEBUG] output: dev=net0, type=0x0800, len=38                       ← (3) デバイスに渡す
[DEBUG] loopback: dev=net0, type=0x0800, len=38                     ← (4) Loopbackで折り返し
[DEBUG] input: dev=net0, type=0x0800, len=38                        ← (5) input_handler
[DEBUG] ip: input: dev=net0, len=38                                 ← (6) IP入力
[DEBUG] ip: accepted: 127.0.0.1 => 127.0.0.1, protocol=1, len=38  ← (7) 宛先OK、上位へ
[DEBUG] icmp: input: 127.0.0.1 => 127.0.0.1, dev=net0, len=18     ← (8) ICMPハンドラ
[DEBUG] icmp: type=8 (Echo), code=0                                 ← (9) Echo Request検出
[INFO ] icmp: Echo Request: id=4660, seq=1
[DEBUG] icmp: output: 127.0.0.1 => 127.0.0.1, type=0 (EchoReply)  ← (10) Echo Reply生成
  ... (Reply が再び Loopback を通って ICMP まで届く) ...
[DEBUG] icmp: type=0 (EchoReply), code=0                            ← (11) Reply受信で停止
```

(2) で len が 18 → 38 に増えているのは、IP ヘッダ (20B) が付加されたため。
(11) で Echo Reply を受信しても、Reply に対しては応答しないのでループは止まる。

---

## Ethernet ヘッダ (`ether.rs`)

### なぜ Phase 2 で必要か

Phase 2 ではまだ Ethernet フレームの送受信はしない（Loopback は Ethernet を使わない）。
しかし定数 (`ETHER_TYPE_IP = 0x0800` 等) とアドレス型は、ARP (Phase 3) や
TAP ドライバ (Phase 3) で即座に必要になるので、ここで定義しておく。

### EtherAddr — MAC アドレスの表現

```rust
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct EtherAddr(pub [u8; 6]);
```

6バイトの配列をラップしただけの newtype パターン。
`Display` で `"aa:bb:cc:dd:ee:ff"` 形式の表示、`FromStr` で文字列からのパースを実装している。

```rust
let addr: EtherAddr = "00:1a:2b:3c:4d:5e".parse().unwrap();
println!("{}", addr); // "00:1a:2b:3c:4d:5e"
```

定数として「全ゼロ (ANY)」と「全 0xff (BROADCAST)」を用意:

```rust
pub const ETHER_ADDR_ANY: EtherAddr = EtherAddr([0; 6]);
pub const ETHER_ADDR_BROADCAST: EtherAddr = EtherAddr([0xff; 6]);
```

### EtherHdr — ゼロコピーパーサ

```rust
pub struct EtherHdr<'a> {
    data: &'a [u8],  // 元のバイト列を借用するだけ
}
```

Ethernet フレームの先頭 14 バイトを借用し、フィールドをオンデマンドで解釈する。
メモリの割り当て (allocation) は一切発生しない。

```
| 0-5 bytes | 6-11 bytes | 12-13 bytes |
| dst MAC   | src MAC    | EtherType   |
```

`new()` で長さチェックし、足りなければ `None` を返す:

```rust
pub fn new(data: &'a [u8]) -> Option<Self> {
    if data.len() < 14 { return None; }
    Some(Self { data })
}
```

この「スライスを借用して on-demand でフィールドを読む」パターンは、
`IpHdr`, `IcmpCommon` でも同じ形で使われている。
TCP/IP スタックでは大量のパケットを処理するため、パースのたびに
構造体にコピーするのは無駄が大きい。

---

## IP 層 (`ip.rs`) — Phase 2 の核

### 構成要素

`ip.rs` は大きく5つの部分で構成されている:

1. **型定義**: `IpAddr`, `IpEndp`, `IpHdr`
2. **論理インタフェース**: `IpIface`
3. **ルーティング**: `IpRoute`, `route_lookup`
4. **パケット入出力**: `input`, `output`, `build_packet`
5. **上位プロトコル登録**: `register_protocol` (ICMP, TCP, UDP を差し込む)

### IpAddr — IPv4 アドレス

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IpAddr(pub [u8; 4]);
```

EtherAddr と同じ newtype パターン。`Display` で `"192.168.1.1"` 形式。

特殊アドレスの定数:

```rust
impl IpAddr {
    pub const ANY: Self = Self([0, 0, 0, 0]);          // 0.0.0.0
    pub const BROADCAST: Self = Self([255, 255, 255, 255]); // 255.255.255.255
}
```

`ANY` は「未指定」を意味し、ルーティングテーブルの nexthop が ANY なら
「直接接続 (on-link)」、送信元が ANY なら「自動でインタフェースのアドレスを使う」
という意味になる。

### IpHdr — IP ヘッダのゼロコピーパーサ

```
 0                   1                   2                   3
 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|Version|  IHL  |    TOS        |         Total Length          |  [0..4]
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|       Identification          |Flags|   Fragment Offset       |  [4..8]
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|      TTL      |   Protocol    |       Header Checksum         |  [8..12]
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                     Source Address                             |  [12..16]
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                  Destination Address                           |  [16..20]
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
```

各フィールドのアクセサは、バイトオフセットから直接読み取る:

```rust
pub fn version(&self) -> u8 { self.data[0] >> 4 }        // 上位4bit
pub fn ihl(&self) -> u8     { self.data[0] & 0x0f }      // 下位4bit
pub fn hlen(&self) -> usize { (self.ihl() as usize) * 4 } // IHL × 4 = ヘッダ長(bytes)
pub fn protocol(&self) -> u8 { self.data[9] }             // 1=ICMP, 6=TCP, 17=UDP
pub fn src(&self) -> IpAddr  { IpAddr([self.data[12], ...]) }
pub fn dst(&self) -> IpAddr  { IpAddr([self.data[16], ...]) }
```

マルチバイトフィールドはネットワークバイトオーダー (big endian):

```rust
pub fn total(&self) -> u16 {
    u16::from_be_bytes([self.data[2], self.data[3]])
}
```

### IpIface — 論理インタフェース

```rust
pub struct IpIface {
    dev: Arc<Device>,    // 紐付くデバイス
    unicast: IpAddr,     // 自分のアドレス (例: 127.0.0.1)
    netmask: IpAddr,     // サブネットマスク (例: 255.0.0.0)
    broadcast: IpAddr,   // ブロードキャストアドレス (例: 127.255.255.255)
}
```

**なぜ Device に直接 IP アドレスを持たせないのか？**

1つのデバイスに複数のプロトコルファミリ (IPv4, IPv6) を紐付ける可能性がある。
`NetIface` トレイトで抽象化し、`family()` で区別する:

```rust
pub trait NetIface: Send + Sync + 'static {
    fn family(&self) -> u16;
    fn as_any(&self) -> &dyn Any;
}
```

`as_any()` は Rust でダウンキャストを実現するための定番パターン。
IP 入力時に `dev.get_iface(FAMILY_IP)` で取得した `Arc<dyn NetIface>` を
`as_any().downcast_ref::<IpIface>()` で具体型に戻す:

```rust
let net_iface = dev.get_iface(FAMILY_IP)?;
let iface = net_iface.as_any().downcast_ref::<IpIface>()?;
// これで iface.unicast() 等にアクセスできる
```

### iface_register — インタフェース登録

```rust
pub fn iface_register(dev: &Arc<Device>, unicast: &str, netmask: &str)
    -> Result<Arc<IpIface>, ()>
```

この関数は3つのことをする:

1. `IpIface` を作成し、`dev.add_iface()` でデバイスに紐付ける
2. グローバルな `IFACES` リストに追加する
3. **サブネットルートを自動追加する**

3番目が重要。たとえば `unicast=127.0.0.1`, `netmask=255.0.0.0` なら:

```
network = 127.0.0.1 & 255.0.0.0 = 127.0.0.0
→ route_add(127.0.0.0, 255.0.0.0, 0.0.0.0, iface)
```

これにより `127.x.x.x` 宛のパケットはこのインタフェースに送られる。

### ルーティングテーブル

```rust
pub struct IpRoute {
    pub network: IpAddr,   // ネットワークアドレス
    pub netmask: IpAddr,   // サブネットマスク
    pub nexthop: IpAddr,   // ゲートウェイ (ANY = 直接接続)
    pub iface: Arc<IpIface>,
}
```

`route_lookup` は最長プレフィクスマッチを行う:

```rust
pub fn route_lookup(dst: IpAddr) -> Option<Arc<IpRoute>> {
    // 全ルートを走査し、マッチするもののうち
    // netmask のビット数 (prefix length) が最大のものを返す
}
```

たとえばルーティングテーブルに以下の2つがあった場合:

```
127.0.0.0/8     → net0 (直接接続)
0.0.0.0/0       → gateway 192.168.1.1 (デフォルトルート)
```

`127.0.0.1` 宛 → prefix 8 が勝ち → `net0` に直接送信
`8.8.8.8` 宛   → prefix 0 にのみマッチ → ゲートウェイ経由

### IP 入力の検証ステップ

`input()` 関数は受信した IP パケットに対して以下の順でチェックする:

1. **最小長チェック** — 20バイト未満なら破棄
2. **バージョンチェック** — IPv4 (= 4) でなければ破棄
3. **ヘッダ長チェック** — data.len() < hlen なら破棄
4. **Total Length チェック** — data.len() < total なら破棄
5. **チェックサム検証** — `cksum16(header) != 0` なら破棄
6. **フラグメントチェック** — フラグメントは未サポート
7. **宛先フィルタリング** — unicast, broadcast, 全ブロードキャスト以外は破棄

すべてパスしたら、`protocol` フィールドに応じて上位プロトコルにディスパッチする。

### IP 出力の流れ

```rust
pub fn output(protocol: u8, data: &[u8], src: IpAddr, dst: IpAddr) -> Result<(), ()>
```

1. `route_lookup(dst)` でルートを検索
2. nexthop を決定 (直接接続 or ゲートウェイ)
3. src が ANY なら、インタフェースの unicast を使う
4. MTU チェック
5. `build_packet()` で IP ヘッダを組み立て
6. `output_device()` でデバイスに渡す

### build_packet — IP ヘッダの組み立て

```rust
fn build_packet(protocol: u8, data: &[u8], src: IpAddr, dst: IpAddr, id: u16)
    -> Result<Vec<u8>, ()>
```

20バイトのヘッダを手動で組み立てる:

```rust
buf[0] = (4 << 4) | 5;           // version=4, IHL=5 (20 bytes)
buf[2..4] = total.to_be_bytes(); // Total Length (big endian)
buf[8] = 255;                    // TTL
buf[9] = protocol;               // 1=ICMP, 6=TCP, 17=UDP
buf[12..16] = src;               // Source Address
buf[16..20] = dst;               // Destination Address
```

チェックサムの計算は、フィールドを全部埋めた後に行う:

```rust
// チェックサムフィールドを0にした状態で計算
let checksum = util::cksum16(&buf[..20], 0);
buf[10..12].copy_from_slice(&checksum.to_ne_bytes());
```

`to_ne_bytes()` (ネイティブエンディアン) を使っているのは、
`cksum16` がネイティブエンディアンで計算結果を返すため。

### 上位プロトコルの登録 (IP 層の二段目ディスパッチ)

Phase 1 では `net::register_protocol(EtherType, handler)` でフレーム種別を振り分けた。
IP 層にも同じ仕組みがある:

```rust
// net.rs: EtherType 0x0800 → ip::input
net::register_protocol(0x0800, ip::input);

// ip.rs: protocol 1 (ICMP) → icmp::input
ip::register_protocol(1, icmp::input);

// 将来: protocol 6 (TCP) → tcp::input
// 将来: protocol 17 (UDP) → udp::input
```

二段階のディスパッチ:

```
Ethernet フレーム受信
  ├── EtherType 0x0800 → ip::input
  │     ├── protocol 1  → icmp::input
  │     ├── protocol 6  → tcp::input   (Phase 5)
  │     ├── protocol 17 → udp::input   (Phase 4)
  │     └── 未知        → ICMP Destination Unreachable
  ├── EtherType 0x0806 → arp::input    (Phase 3)
  └── 未知 → 無視
```

---

## ICMP (`icmp.rs`)

### 何をやっているか

ICMP (Internet Control Message Protocol) は IP 層のエラー通知と診断に使う。
Phase 2 で実装するのは2つ:

1. **Echo Reply** — `ping` コマンドの応答
2. **Destination Unreachable** — 未知の上位プロトコル宛パケットのエラー通知

### ICMP ヘッダ

```
 0                   1                   2                   3
 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|     Type      |     Code      |          Checksum             |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                  Type-Dependent (id+seq or unused)            |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
```

先頭 8 バイトが共通ヘッダ。Type=8 が Echo Request、Type=0 が Echo Reply。
Echo の場合、4〜5 バイト目が Identifier、6〜7 バイト目が Sequence Number。

### Echo Request → Echo Reply

`input()` の中核:

```rust
if com.ty() == ICMP_TYPE_ECHO {
    output(
        ICMP_TYPE_ECHO_REPLY,  // Type: 0
        0,                      // Code: 0
        com.dep(),              // id + seq をそのまま返す
        &data[8..],             // ペイロードもそのまま返す
        iface.unicast(),        // src: 自分のアドレス
        hdr.src(),              // dst: 送信元に返す
    );
}
```

`com.dep()` は Type-Dependent フィールド (4バイト) をそのまま u32 で返す。
Echo の場合これは id (2B) + seq (2B) なので、Reply にそのままコピーすることで
「どの ping に対する応答か」を送信側が識別できる。

### Destination Unreachable

IP 入力で未知のプロトコル番号を受信した場合:

```rust
// ip.rs の input() 内
if handler.is_none() {
    icmp::output(
        ICMP_TYPE_DEST_UNREACH,    // Type: 3
        ICMP_CODE_PROTO_UNREACH,   // Code: 2 (Protocol Unreachable)
        0,
        &data[..icmp_data_len],    // 元の IP ヘッダ + 先頭 8 バイト
        iface.unicast(),
        hdr.src(),
    );
}
```

RFC 792 の規定により、Destination Unreachable メッセージには
「問題を起こした元パケットの IP ヘッダ + 先頭 8 バイト」を含める。
これにより送信側が「どのパケットに対するエラーか」を特定できる。

### ICMP チェックサム

IP チェックサムと同じ RFC 1071 アルゴリズムだが、範囲が異なる:

- **IP チェックサム**: IP ヘッダのみ (20〜60 バイト)
- **ICMP チェックサム**: ICMP ヘッダ + データ全体

```rust
// 送信時: チェックサムフィールドを 0 にして全体を計算
buf[2..4] = [0, 0];
let sum = cksum16(&buf, 0);
buf[2..4] = sum.to_ne_bytes();

// 受信時: 全体のチェックサムが 0 になるか検証
if cksum16(data, 0) != 0 { /* エラー */ }
```

---

## Device の変更点 (`device.rs`)

### NetIface トレイト

Phase 1 では `Device` は純粋なリンク層の抽象化だった。
Phase 2 で「IP インタフェース」という概念を `Device` に紐付ける必要が生じた。

```rust
pub trait NetIface: Send + Sync + 'static {
    fn family(&self) -> u16;       // FAMILY_IP = 1, FAMILY_IPV6 = 2
    fn as_any(&self) -> &dyn Any;  // ダウンキャスト用
}
```

`Device` に `ifaces` フィールドを追加:

```rust
pub struct Device {
    // ... Phase 1 のフィールド ...
    ifaces: Mutex<Vec<Arc<dyn NetIface>>>,  // ← 追加
}
```

**なぜ `Arc<dyn NetIface>` か**: `IpIface` はルーティングテーブルからも参照されるため、
複数のオーナーが必要。`Arc` で共有所有権を持たせる。

### as_any パターン

Rust のトレイトオブジェクト (`dyn NetIface`) は具体型の情報を持たないため、
フィールドに直接アクセスできない。`as_any()` + `downcast_ref()` で具体型に戻す:

```rust
// 取得時
let net_iface: Arc<dyn NetIface> = dev.get_iface(FAMILY_IP)?;

// ダウンキャスト
let iface: &IpIface = net_iface.as_any().downcast_ref::<IpIface>()?;

// これで IP 固有のフィールドにアクセスできる
iface.unicast()  // → 127.0.0.1
```

この `Any` を使ったダウンキャストは microps-rs でも同じパターンが使われている。
Rust の型システムの制約を回避するための定番テクニック。

---

## net.rs の変更点

`init()` で IP と ICMP を初期化するようになった:

```rust
pub fn init() -> Result<(), ()> {
    crate::ip::init()?;    // net に 0x0800 ハンドラを登録
    crate::icmp::init()?;  // ip に protocol=1 ハンドラを登録
    Ok(())
}
```

呼び出し順が重要: `ip::init()` が先でないと `icmp::init()` の中で
`ip::register_protocol()` が呼べない（IP のプロトコル登録テーブルは ip::init とは
独立した static なので実際には順序不問だが、論理的な依存関係を明示するために
この順序にしている）。

---

## Phase 1 との対応

| Phase 1 の仕組み | Phase 2 で実現されたこと |
|---|---|
| `net::register_protocol(ty, handler)` | `ip::init()` が `register_protocol(0x0800, ip::input)` を呼ぶ |
| `net::input_handler()` → ディスパッチ | EtherType 0x0800 → `ip::input` が呼ばれる |
| `device::Ops::transmit()` | `ip::output_device()` → `dev.output()` → `Ops::transmit()` |
| `device::Device` の抽象化 | `IpIface` がデバイスに紐付き、IP 固有情報を保持 |

Phase 1 で作った「プロトコルをプラグインする基盤」が、
Phase 2 で IP と ICMP という実際のプロトコルを差し込むことで活きている。

---

## Phase 3 に向けて

Phase 2 では Loopback 経由でしか通信できない。
Phase 3 では以下を実装し、ホスト OS からの `ping` に応答できるようにする:

- **TAP ドライバ** — Linux の仮想 NIC (`/dev/net/tun`) を使って実際のパケットを送受信
- **ARP** — IP アドレスから MAC アドレスを解決する
- **Ethernet 入出力** — `ether.rs` の型を使って実際のフレームを組み立て・解析

Phase 3 が完了すると、別のターミナルから `ping 192.168.x.x` を打って
自作スタックが応答する、という体験ができる。
