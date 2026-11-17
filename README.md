# ltentp

LTE 回線の網時刻 (NITZ) を、GNSS NTP のような時刻源として使えるかを評価するための PoC です。
WWAN モデムの AT ポートから時刻関連の通知を読み、受信した瞬間のシステム時刻と一緒に CSV に記録し、`+CTZEU` (UTC 付きの網時刻) を chrony の `refclock SOCK` に送ります。

評価機のシステム時刻は、chrony で有線 LAN の GNSS NTP (基準) に合わせておきます。NITZ と LTE 経由の NTP は `noselect` で観測するだけで、時計の調整には使いません。
そのため CSV の `offset_s` は「NITZ − GNSS 基準」の近似になります。意味を持つのは、chrony が基準に同期している間 (`chronyc tracking` で確認) の値だけです。

## 状態

- ハードウェアなしのテスト (ユニットテスト、FIFO と偽 chronyd を使う結合テスト) は通っています
- **実機 (Debian + L860-GL) ではまだ動かしていません**。`contrib/` の設定例も未検証です
- 想定する実機: ThinkPad P14s Gen 4 の Fibocom L860-GL (PCI `8086:7560`、Linux では `iosm` ドライバ)

## 必要なもの

- Linux (Debian 13 trixie を想定)。Windows ではビルドできません (開発は WSL の Debian で行います)
- Rust 1.85 以降 (edition 2024)。外部クレートは使っていません
- 実機で使う場合:
  - `iosm` ドライバで `/dev/wwan0at0`・`/dev/wwan0at1` が見えていること
  - FCC unlock 済みであること (Lenovo の `lenovo-wwan-unlock`、または ModemManager の `8086:7560` スクリプト)
  - ModemManager に使わせない AT ポート 1 本 (`contrib/udev/` 参照)
  - chrony (`refclock SOCK` を使う場合)

## ビルドとテスト

```sh
bash scripts/check.sh   # cargo fmt --check / clippy -D warnings (pedantic) / test
cargo build --release
```

Windows から WSL で実行する場合:

```powershell
wsl -d Debian --cd /mnt/c/repository/ltentp --exec bash -l scripts/check.sh
```

## 使い方

```sh
ltentp --device /dev/wwan0at1 [--chrony-sock /run/chrony/ltentp-nitz.sock] [--csv nitz.csv] [--cclk-interval 60]
```

- `--device`: AT ポート。TTY ではなく WWAN の文字デバイスなので、通常のファイルとして開きます (通信速度などの設定はしません)
- `--chrony-sock`: chrony の `refclock SOCK` のパス。chronyd が作ったソケットに送るだけで、ltentp 側では作りません。送信に失敗しても記録は続けます
- `--csv`: CSV の出力先 (追記)。省略時は標準出力。空のファイルのときだけ見出し行を書きます
- `--cclk-interval`: N 秒ごとに `AT+CCLK?` も送り、モデムの時計とのずれも記録します (既定 0 = 送らない)
- 存在しないオプションや重複指定はエラーで、終了コード 2 になります

起動時には毎回 `AT+CTZR=3` を送り、`+CTZEU` の通知を有効にします。この機体のファームウェア (18601.5001.00.01.17.30) が `+CTZR: (0-3)` を返し、3 に対応していることは Windows 上で確認済みです。
網を登録し直させて NITZ を強制的に送らせる操作 (`AT+CFUN`・`AT+COPS` など) はしません。LTE のデータ通信が切れ、LTE 経由 NTP の測定を乱すためです。

デバイスと chrony のソケットに書き込むため、PoC の段階では root で実行する想定です。

## CSV の列

受信したすべての行 (エコー、`OK`、解析できなかった行を含む) を 1 行ずつ記録します。

| 列 | 内容 |
|---|---|
| `mono_s` | 起動からの単調時間 (秒)。時計の補正の影響を受けない |
| `realtime_s` | read が返った直後のシステム時刻 (Unix 秒、ナノ秒まで) |
| `kind` | `CTZEU` / `CTZE` / `CTZV` / `CCLK` / `LINE` (その他の行) / `PARSE_ERROR` |
| `tz_quarters` | タイムゾーン (15 分単位。+36 = +9 時間) |
| `dst` | 夏時間の補正 (0〜2) |
| `remote_unix_s` | 網またはモデムが示した時刻 (Unix 秒、UTC として解釈) |
| `offset_s` | `remote_unix_s` − `realtime_s`。正ならシステム時刻が遅れている (chrony の offset と同じ向き)。システム時刻が GNSS 基準に同期している間は「NITZ − 基準」とみなせる |
| `chrony` | `sent`、`error: ...`、または空 (送信対象外か `--chrony-sock` なし) |
| `note` | 解析エラーの理由 |
| `raw` | 受信した行そのもの |

1 回の read で届いた行は、すべて同じ時刻で記録します。
NITZ の時刻は秒単位で切り捨てられているため、`offset_s` は 0〜−1 秒の偏りと配送遅延を含んだ生の値です。補正はしていません。

`CCLK` 行は記録だけで chrony には送りません。この機体では時刻欄が UTC と一致し、タイムゾーン欄だけが +9 時間を示していたため、意味が確定していないからです (`docs/at-commands.md`)。

## 設定例 (実機で未検証)

- `contrib/chrony/ltentp.conf`: `/etc/chrony/conf.d/` に置く chrony の設定
  - chrony のソース (`refclock.c`・`samplefilt.c`) で確認したところ、参照クロックのフィルタは 1 回の poll にサンプルが 1 つあれば値を出します。NITZ がまれにしか届かなくても、既定の `filter` のままで chrony の統計に載ります
  - `log refclocks` は、受け取ったサンプルを受信のたびに記録します
- `contrib/udev/77-ltentp-mm-ignore.rules`: `wwan0at1` を ModemManager に使わせない udev ルール

## ドキュメント

- `docs/at-commands.md`: 使う AT コマンドと応答形式、実機で観測したこと
- `docs/chrony-sock.md`: chrony `refclock SOCK` に送るデータの形式
