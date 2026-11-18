# ltentp

LTE のモジュールや LTE のやり取り (NITZ・SIB16 など網からの情報) から時刻を取り、GNSS NTP のような時刻源として使えるかを評価するための PoC です。LTE 回線の上で NTP を使うことは対象外です。
WWAN モデムの AT ポートから時刻関連の通知を読み、受信した瞬間のシステム時刻と一緒に CSV に記録し、`+CTZEU` (UTC 付きの網時刻) を chrony の `refclock SOCK` に送ります。
あわせて `AT+CCLK?` を短い間隔で問い合わせ、モデムの時計の秒が切り替わる瞬間を数 ms の幅で求めます (`CCLK_EDGE`)。

評価機のシステム時刻は、chrony で有線 LAN の NTP (基準) に合わせておきます。NITZ は `noselect` で観測するだけで、時計の調整には使いません。
そのため CSV の `offset_s` は「NITZ − GNSS 基準」の近似になります。意味を持つのは、chrony が基準に同期している間 (`chronyc tracking` で確認) の値だけです。

## 状態

- ハードウェアなしのテスト (ユニットテスト、FIFO と偽 chronyd を使う結合テスト) は通っています
- 実機: ThinkPad P14s Gen 4 (21HF) の Fibocom L860-GL-16 (`AT+CGMM`、Intel XMM7560、PCI `8086:7560`、サブシステム `1cf8:8655`、ファームウェア 18601.5001.00.01.17.30)、Debian 13.7 (カーネル 6.12、`iosm` ドライバ)、IIJmio (KDDI 網)
- 2026-11-18 に実機で確認したこと (`docs/at-commands.md`):
  - FCC unlock、網への登録、AT ポートからの読み取りと `AT+CTZR=3` は動きました
  - **`+CTZEU` は一度も届いていません** (初回の登録を含む)。chrony に NITZ を送る経路は実機では未検証です
  - `--cclk-interval` (`CCLK_EDGE`) は実機で動き、秒の切り替わりを ±6〜8 ms の幅で求められました。モデムの時計と UTC のずれは、登録の直後は −0.56 秒・−0.82 秒、1 回目の起動で登録から約 1 時間後は +10 ms でした。1 秒より細かい網時刻が入るのか、入るならいつかは分かっていません (`docs/at-commands.md`)
- `contrib/` の設定例は未検証です

## 必要なもの

- Linux (Debian 13 trixie を想定)。Windows ではビルドできません (開発は WSL の Debian で行います)
- Rust 1.85 以降 (edition 2024)。外部クレートは使っていません
- 実機で使う場合:
  - `iosm` ドライバで `/dev/wwan0at0`・`/dev/wwan0at1` が見えていること
  - FCC unlock 済みで、網に登録されていること。起動直後のモデムは `+CFUN: 4` (無線オフ)・`+COPS: 2` (登録解除) なので、unlock のあと `AT+CFUN=1` と `AT+COPS=0` が必要です (ltentp は送りません。手順は `docs/at-commands.md`)
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
ltentp --device /dev/wwan0at1 [--chrony-sock /run/chrony/ltentp-nitz.sock] [--csv nitz.csv] \
       [--cclk-interval 10 [--cclk-step 10]]
```

- `--device`: AT ポート。TTY ではなく WWAN の文字デバイスなので、通常のファイルとして開きます (通信速度などの設定はしません)
- `--chrony-sock`: chrony の `refclock SOCK` のパス。chronyd が作ったソケットに送るだけで、ltentp 側では作りません。送信に失敗しても記録は続けます
- `--csv`: CSV の出力先 (追記)。省略時は標準出力。空のファイルのときだけ見出し行を書きます
- `--cclk-interval`: N 秒 (整数) ごとに、モデムの時計の秒の切り替わりを 1 回測ります (既定 0 = 測らない)。`--cclk-step` ミリ秒間隔で `AT+CCLK?` を送り、切り替わりが見つかるまで続けます (最長 1.2 秒)。2 回目からは前回の切り替わりから予測した時刻の 0.1 秒前に始めるので、1 回あたりの問い合わせは十数回です
- `--cclk-step`: 上の問い合わせの間隔 (1〜1000 ms、既定 10)。応答を受け取ってから次を送るので、実際の間隔は応答時間 (実機で約 4.5 ms) より短くなりません
- 存在しないオプションや重複指定はエラーで、終了コード 2 になります

起動時には毎回 `AT+CTZR=3` を送り、`+CTZEU` の通知を有効にします。この機体のファームウェア (18601.5001.00.01.17.30) が `+CTZR: (0-3)` を返し、3 に対応していることは確認済みです。
網を登録し直させて NITZ を強制的に送らせる操作 (`AT+CFUN`・`AT+COPS` など) はしません。網から切断する操作は、実験として手で行います。

デバイスと chrony のソケットに書き込むため、PoC の段階では root で実行する想定です。

## CSV の列

受信したすべての行 (エコー、`OK`、解析できなかった行を含む) を 1 行ずつ記録します。

| 列 | 内容 |
|---|---|
| `mono_s` | 起動からの単調時間 (秒)。時計の補正の影響を受けない |
| `realtime_s` | read が返った直後のシステム時刻 (Unix 秒、ナノ秒まで) |
| `kind` | `CTZEU` / `CTZE` / `CTZV` / `CCLK` / `CCLK_EDGE` (秒の切り替わり、下記) / `LINE` (その他の行) / `PARSE_ERROR` |
| `tz_quarters` | タイムゾーン (15 分単位。+36 = +9 時間) |
| `dst` | 夏時間の補正 (0〜2) |
| `remote_unix_s` | 網またはモデムが示した時刻 (Unix 秒、UTC)。`CCLK` は時刻欄からタイムゾーンを引いた値 |
| `offset_s` | `remote_unix_s` − `realtime_s`。正ならシステム時刻が遅れている (chrony の offset と同じ向き)。システム時刻が GNSS 基準に同期している間は「NITZ − 基準」とみなせる |
| `chrony` | `sent`、`error: ...`、または空 (送信対象外か `--chrony-sock` なし) |
| `note` | 解析エラーの理由。`CCLK_EDGE` では `halfwidth_s=` (誤差幅の半分)。時計が未設定とみられる行 (システム時刻と 1 日以上違う) にはその旨 |
| `raw` | 受信した行そのもの |

1 回の read で届いた行は、すべて同じ時刻で記録します。
NITZ の時刻は秒単位で切り捨てられているため、`offset_s` は 0〜−1 秒の偏りと配送遅延を含んだ生の値です。補正はしていません。

`CCLK` の時刻欄は、TS 27.007 どおり現地時刻としてタイムゾーンを引き UTC にします。Linux の実機 (2026-11-18) ではこの解釈で正しい時刻になりました。2026-11-17 に Windows で見たときは時刻欄が UTC と一致していて、食い違いの原因は分かっていません (`docs/at-commands.md`)。
モデムを起動した直後は時計が `70/01/01` から数えていて、2 桁の年を 20yy と読むため 2070 年になります。システム時刻と 1 日以上違う行は `offset_s` を空にして、`note` に未設定の印を付けます。

`CCLK` と `CCLK_EDGE` は記録するだけで chrony には送りません。モデムの時計を網が何で合わせたか (NITZ か、より細かい情報か) が分かっていないためです。

`CCLK_EDGE` の求め方: 秒の欄が `S` の応答の直後に `S+1` の応答が来たら、モデムの時計はその前の問い合わせを送った時刻 (`lo`) から後の応答を受け取った時刻 (`hi`) までの間に `S+1` になったはずです。行の時刻 (`mono_s`・`realtime_s`) はその中点、`offset_s` は `S+1` − 中点、`note` の `halfwidth_s` は (`hi` − `lo`) / 2 です。モデムが問い合わせのどの時点で時計を読むかは分からないため、中点を推定値にしています。

## 設定例 (実機で未検証)

- `contrib/chrony/ltentp.conf`: `/etc/chrony/conf.d/` に置く chrony の設定
  - chrony のソース (`refclock.c`・`samplefilt.c`) で確認したところ、参照クロックのフィルタは 1 回の poll にサンプルが 1 つあれば値を出します。NITZ がまれにしか届かなくても、既定の `filter` のままで chrony の統計に載ります
  - `log refclocks` は、受け取ったサンプルを受信のたびに記録します
- `contrib/udev/77-ltentp-mm-ignore.rules`: `wwan0at1` を ModemManager に使わせない udev ルール

## ドキュメント

- `docs/at-commands.md`: 使う AT コマンドと応答形式、実機で観測したこと
- `docs/chrony-sock.md`: chrony `refclock SOCK` に送るデータの形式
