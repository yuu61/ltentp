# ltentp

LTE 回線の網時刻 (NITZ) を、GNSS NTP のような時刻源として使えるかを評価するための PoC です。
WWAN モデムの AT ポートから時刻関連の通知を読み、受信した瞬間のシステム時刻と一緒に CSV に記録し、`+CTZEU` (UTC 付きの網時刻) を chrony の `refclock SOCK` に送ります。
あわせて `AT+CCLK?` を短い間隔で問い合わせ、モデムの時計の秒が切り替わる瞬間を数 ms の幅で求めます (`CCLK_EDGE`)。

評価機のシステム時刻は、chrony で有線 LAN の GNSS NTP (基準) に合わせておきます。NITZ と LTE 経由の NTP は `noselect` で観測するだけで、時計の調整には使いません。
そのため CSV の `offset_s` は「NITZ − GNSS 基準」の近似になります。意味を持つのは、chrony が基準に同期している間 (`chronyc tracking` で確認) の値だけです。

## 状態

- ハードウェアなしのテスト (ユニットテスト、FIFO と偽 chronyd を使う結合テスト) は通っています
- 実機: ThinkPad P14s Gen 4 の Fibocom L860-GL (PCI `8086:7560`、Linux では `iosm` ドライバ)、Debian 13、IIJmio (KDDI 網)
- 2026-11-18 に実機で確認したこと (`docs/at-commands.md`):
  - FCC unlock、網への登録、AT ポートからの読み取りと `AT+CTZR=3` は動きました
  - **`+CTZEU` は一度も届いていません** (初回の登録を含む)。chrony に NITZ を送る経路は実機では未検証です
  - `--cclk-interval` (`CCLK_EDGE`) は実機で動き、秒の切り替わりを ±6〜8 ms の幅で求められました。モデムの時計と UTC のずれは、登録の直後は −0.56 秒・−0.82 秒、1 回目の起動で登録から約 1 時間後は +10 ms でした。1 秒より細かい網時刻が入るのか、入るならいつかは分かっていません (`docs/at-commands.md`)
  - LTE 経由の NTP (IIJmio、`wwan0` へのホスト経路) は動いています。NICT までの往復は LAN 経由で約 10 ms、LTE 経由で約 62 ms、LTE 経由のずれは +8〜+16 ms (上りと下りの遅延の差によるもの) でした (2026-11-18、数回の測定)
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
網を登録し直させて NITZ を強制的に送らせる操作 (`AT+CFUN`・`AT+COPS` など) はしません。LTE のデータ通信が切れ、LTE 経由 NTP の測定を乱すためです。

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

## LTE 経由の NTP (実機で使った手順)

ModemManager を使わず、libmbim-utils (`mbimcli`) でつなぎます。網への登録が済んでいる前提です。

```sh
systemd-run --unit=mbim-proxy-ltentp /usr/libexec/mbim-proxy --no-exit   # MBIM を開いたままにする
mbimcli -d /dev/wwan0mbim0 -p --attach-packet-service
mbimcli -d /dev/wwan0mbim0 -p --connect="session-id=0,access-string=iijmio.jp,ip-type=ipv4,auth=chap,username=mio@iij,password=iij"
# 表示された IPv4 アドレスと MTU を設定する。既定経路と DNS は変えない
ip addr add <address>/<prefix> dev wwan0
ip link set wwan0 mtu <mtu> up
sysctl -w net.ipv6.conf.wwan0.accept_ra=0   # IPv6 の既定経路が LTE に移らないように
# 比べる NTP サーバーだけ wwan0 に向ける。wwan0 が落ちたときに LAN へ漏れないよう unreachable も足す
ip route add 133.243.238.164/32 dev wwan0 src <address>
ip route add unreachable 133.243.238.164/32 metric 4000
# chrony には実行中だけ足す (再起動で経路と一緒に消える)
chronyc add server 133.243.238.164 iburst noselect minpoll 6 maxpoll 6
```

- APN 等は IIJmio の公式の設定値です。`auth` は小文字 (`chap`) でないと `mbimcli` が受け付けません
- 経路が LTE を通ることは `ip route get <サーバー>` と `/sys/class/net/wwan0/statistics/` の増加で確かめます
- LTE 経由のサーバーを chrony の設定ファイルに書くと、再起動後は経路がないまま LAN 経由で測ってしまいます。比べる相手の LAN 経由のサーバー (同じ NICT の別アドレス) と `log measurements` は設定ファイルに書きます (`contrib/chrony/ltentp.conf`)

## ドキュメント

- `docs/at-commands.md`: 使う AT コマンドと応答形式、実機で観測したこと
- `docs/chrony-sock.md`: chrony `refclock SOCK` に送るデータの形式
