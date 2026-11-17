# chrony `refclock SOCK` に送るデータ

出典: chrony の `refclock_sock.c` (`struct sock_sample`)。

chronyd が設定のパスに Unix ドメインのデータグラムソケットを作って待ち受けます。ltentp はパスに bind せず、`send_to` で 1 サンプル 40 バイトを送るだけです。chronyd が動いていなければ送信は失敗しますが、ltentp は CSV に `error: ...` と記録して動き続けます。

## 形式 (64 ビット Linux、ネイティブエンディアン)

| オフセット | フィールド | 型 | ltentp が入れる値 |
|---|---|---|---|
| 0 | `tv.tv_sec` | i64 | 受信時のシステム時刻 (秒) |
| 8 | `tv.tv_usec` | i64 | 同 (マイクロ秒) |
| 16 | `offset` | f64 | 真の時刻 − システム時刻 (秒) = NITZ の UTC − 受信時刻 |
| 24 | `pulse` | i32 | 0 |
| 28 | `leap` | i32 | 0 |
| 32 | `_pad` | i32 | 0 |
| 36 | `magic` | i32 | `0x534f434b` |

`offset` の符号は、システム時刻が遅れていれば正です。テスト (`chrony::tests::offset_is_reference_minus_system`) で両方の符号を確認しています。

32 ビット環境では `timeval` の大きさが変わるため、ビルドエラーにしています。
