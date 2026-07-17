# simple-rust-check

Codex CLI（rustls + aws-lc-rs）と同一の TLS スタックで Cisco MCP-GW にリクエストを送り、
直接接続で失敗するのが **rustls の TLS 指紋（JA3）に起因するか** を検証する最小プログラム。

## TLS スタック（Codex と同一）
- `reqwest 0.12` + `default-features = false` + `rustls-tls-native-roots`
- プロバイダ: aws-lc-rs
- ルート証明書: OS の証明書ストア（rustls-native-certs）

## 実行（Windows）

### 1. Rust ツールチェーンのインストール（未導入の場合）
PowerShell:
```powershell
winget install Rustlang.Rustup
```
または https://rustup.rs/ 。インストール後、新しいターミナルを開く。

### 2. clone & 実行
```cmd
git clone https://github.com/pcparts001/simple-rust-check.git
cd simple-rust-check
cargo run
```

> ※ Fiddler 等のプロキシは **切って**（`HTTPS_PROXY` 未設定のまま）直接接続で実行してください。

## 判定

2 つのテスト（GET / POST initialize）の結果を見て:

- **両方 `OK: status=401`** → rustls でも gateway は 401 を返す。Codex の失敗は別要因。
- **いずれかが `ERR: ...`（timeout 等）** → **rustls の TLS 指紋が原因で確定**。

curl（Schannel）では直接接続で 401 が返るため、rustls だけ挙動が違えば TLS 指紋説が確定します。
