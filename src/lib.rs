// my-tcpip: 自作TCP/IPプロトコルスタック
//
// Phase 1: 基盤 (step 0-3)
//   step 0: ライフサイクル管理 (net::init / run / shutdown)
//   step 1: デバイス管理 (Device 構造体、登録、open/close)
//   step 2: Loopback ドライバ
//   step 3: プロトコル登録とディスパッチ

pub mod device;
pub mod driver;
pub mod net;
pub mod util;
