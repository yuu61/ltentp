# CLAUDE.md

LTE の網時刻 (NITZ) を時刻源として評価する PoC。Rust、Linux 専用。目的と使い方は README.md。

## Commands

Windows から実行する場合は WSL の Debian を使う (Windows ではビルドできない):

```powershell
# fmt --check / clippy -D warnings (pedantic) / test (ユニット + tests/e2e.rs)
wsl -d Debian --cd /mnt/c/repository/ltentp --exec bash -l scripts/check.sh
# 整形
wsl -d Debian --cd /mnt/c/repository/ltentp --exec bash -l -c 'cargo fmt'
```

- `--exec` を使う。`wsl -d Debian -- bash -lc '...'` は間に別のシェルが入り、`$?` などが先に展開される
- `bash -l` が必要 (cargo は `~/.cargo/bin` にあり、ログインシェルで PATH に入る)
- 上のコマンドは PowerShell 用。Git Bash から実行すると `/mnt/c/...` が Windows のパスに書き換えられて失敗する (`MSYS_NO_PATHCONV=1` を付ける)
- 終了コードは `$LASTEXITCODE` で取れる (確認済み)
- ビルド出力は `CARGO_TARGET_DIR=$HOME/.cache/ltentp-target` (WSL 側)。`/mnt/c` は遅いため
- WSL には python3・socat・chrony は入っていない。入れる前にユーザーに確認する

## Architecture

- `src/framing.rs`: バイト列を行に分割 (CR/LF、読み込みをまたぐ行)
- `src/at.rs`: `+CTZEU`/`+CTZE`/`+CTZV`/`+CCLK` の解析 (引用符の外のカンマだけで区切る)
- `src/civil.rs`: 日時と Unix 秒の変換 (ゾーンは適用しない)
- `src/clock.rs`: read 直後の時刻 (システム時刻 + 単調時間)。テストでは差し替える
- `src/chrony.rs`: `struct sock_sample` (40 バイト) の組み立てと送信
- `src/recorder.rs`: 行 → CSV と chrony 送信、読み込みループ `run`
- `src/cli.rs` / `src/main.rs`: オプション解析と起動 (`AT+CTZR=3` 送信、`AT+CCLK?` の定期送信)

## Conventions

- 外部クレートは使わない。追加する場合は理由を README.md に書く
- `unsafe` 禁止 (`Cargo.toml` の `[lints]`)。clippy pedantic を警告なしで通す
- 受信した行はすべて CSV に残す。解析に失敗しても止めない
- chrony の offset は「真の時刻 − システム時刻」。符号を変える変更ではテストで両方の符号を確認する
- `+CCLK` はゾーンを適用せず、chrony にも送らない (docs/at-commands.md)
- `AT+CFUN`・`AT+COPS` など網から切断する操作を既定で行わない
- コードのコメントは英語、ドキュメントは日本語
- AT の仕様を足すときは docs/at-commands.md も更新する。Fibocom のマニュアルは転載禁止なので要約して節とページを書く
- 実機で確認していないことは「未検証」と明記する
