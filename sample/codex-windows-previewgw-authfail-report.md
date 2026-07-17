# Codex Windows版 — Cisco MCP-GW (preview.aidefense) 認証エラー 調査レポート

- **対象**: Codex CLI 0.144.5 (Windows) × Streamable HTTP 型 MCP サーバー
- **症状**: MCP サーバー `employee-information` の OAuth 認証・接続が失敗
- **根本原因**: Windows 版 Codex の MCP HTTP パスが **Schannel (native-tls)** を使い、その Schannel が gateway と TLS ハンドシェイクで失敗する（`SEC_E_ILLEGAL_MESSAGE`）
- **解決**: `SSL_CERT_FILE` / `CODEX_CA_CERTIFICATE` で rustls を強制 → ログイン成功・tool call 成功を確認済み
- **確認日**: 2026-07-18

---

## 1. 概要

install.ps1 でインストールした Windows 版 Codex CLI で、Streamable HTTP 型 MCP サーバー（Cisco MCP-GW: `gateway.agent.preview.aidefense.aiteam.cisco.com`）の OAuth 認証と initialize が失敗する。同じ Windows で curl / 自作 Python クライアント（OpenSSL）は成功、Mac 版 Codex も成功するという報告があった。

## 2. 症状

- `codex mcp login employee-information`
  → `Error: No authorization support detected`
- `codex`（TUI 起動時）
  → `MCP client for 'employee-information' failed to start: ... error sending request for url (https://gateway...)`

## 3. 根本原因

### 3.1 エラーの正体

RUST_LOG に `codex_exec_server=debug` を追加して取得した決定ログ:

```
WARN codex_exec_server::client::http_client::reqwest_http_client:
  http/request send failed
  http_method="GET"
  error_is_timeout=false
  error_is_connect=true                          ← TLS 接続（ハンドシェイク）エラー
  error=error sending request
  error_sources="client error (Connect): The message received was unexpected or badly formatted. (os error -2146893018)"
```

`os error -2146893018` = `0x80090326` = **`SEC_E_ILLEGAL_MESSAGE`（Windows Schannel の TLS プロトコルエラー）**。

これは rustls のエラーではなく **Windows ネイティブ TLS（Schannel）のエラー**。TCP 接続は成功（`connected to 52.43.88.253:443`）した直後の TLS ハンドシェイクで失敗し、HTTP レスポンス（401 等）すら届かない。そのため discovery がすべて失敗し `NoAuthorizationSupport` → `No authorization support detected` となる。

### 3.2 なぜ Schannel を使っているか

Codex の `codex-rs/http-client/src/custom_ca.rs` の `build_reqwest_client_with_env` は、**カスタム CA（`SSL_CERT_FILE` / `CODEX_CA_CERTIFICATE`）が未設定の場合、`builder.build()` だけで `use_rustls_tls()` を呼ばない。**

- reqwest は workspace の Cargo.toml で `default-features` を無効にしていない → `default-tls`（= native-tls）が有効
- `exec-server` / `http-client` は `rustls-tls` feature を追加するが、**reqwest は複数 TLS feature が有効な場合 native-tls を優先する**
- カスタム CA ありパス（`custom_ca.rs:300-307`）では `use_rustls_tls()` を呼んで rustls を強制するが、**なしパスではプラットフォームのネイティブ TLS が使われる**

結果として Windows では `schannel` crate（Rust から Schannel SSPI を叩く実装）が選ばれる。

## 4. なぜ Windows だけで Mac/Linux は成功するか

| プラットフォーム | native-tls 実装 | gateway との TLS | 結果 |
|---|---|---|---|
| **Windows** | **Schannel (schannel crate)** | **SEC_E_ILLEGAL_MESSAGE** | **失敗 ❌** |
| macOS | Secure Transport | 成功 | 成功 ✅ |
| Linux | OpenSSL | 成功 | 成功 ✅ |

`schannel` crate の TLS 実装（暗号スイート、拡張の解釈、TLS 1.3 の扱い）が、この Cisco MCP-GW（AWS API Gateway/CloudFront 系、IdenTrust/HydrantID 証明書）と非互換。Mac の Secure Transport / Linux の OpenSSL / rustls / curl の Schannel（独自実装）はいずれも成功する。**Windows の `schannel` crate だけが失敗する。**

「Mac 版 Codex は成功」の報告と完全に整合する。

## 5. 各クライアントの結果（辻褄合わせ）

| クライアント | TLS バックエンド | 直接接続の結果 |
|---|---|---|
| Codex (Windows) | Schannel (schannel crate) | SEC_E_ILLEGAL_MESSAGE ❌ |
| Codex (Mac) | Secure Transport | 成功 ✅ |
| テストプログラム (rustls 強制) | rustls | 401 ✅ |
| curl | Schannel (独自実装) | 401 ✅ |
| simple-mcp-client (Python) | OpenSSL | 401 ✅ |
| Codex (Fiddler 経由) | Fiddler の TLS (MITM 再終端) | 成功 ✅ |

Fiddler 経由で成功したのは、Fiddler が TLS を再終端するため（Codex↔Fiddler は localhost の Schannel で成功、Fiddler↔gateway は Fiddler の TLS クライアントで成功）。

## 5.5 補足: なぜ Fiddler 経由で成功したか

Fiddler は HTTPS を傍受するため **TLS を再終端（MITM）** するプロキシ。`HTTPS_PROXY=http://127.0.0.1:8888` を設定すると、Codex の通信は Fiddler を経由し、TLS セッションが **2 つに分かれる**:

```
Codex(Schannel)  ←TLS①→  Fiddler  ←TLS②→  gateway
      ↑                          ↑
 localhost(127.0.0.1)       本物の gateway
 Fiddlerの偽証明書          gatewayの本物証明書
```

- **TLS①（Codex ↔ Fiddler）**: Codex の Schannel は **localhost の Fiddler** とだけハンドシェイクする。Fiddler は自身のルート CA で「gateway のドメイン名を持つ偽証明書」を発行し、Fiddler の CA は Windows 証明書ストアに信頼済み（Fiddler インストール時）。しかも相手は localhost の Fiddler なので、Codex の Schannel は **gateway とは無関係**。Schannel と Fiddler は問題なく握手できる。→ 成功
- **TLS②（Fiddler ↔ gateway）**: gateway とのハンドシェイクは **Fiddler の TLS クライアント** が担当し、Codex の Schannel は一切関与しない。Fiddler の TLS 実装は gateway と互換。→ 成功

つまり **Codex の Schannel は gateway と直接通信しなくなる**。Fiddler が間に入って TLS を終端し、別の TLS セッションで gateway に再接続するため、Codex 側の Schannel 非互換は完全にバイパスされる。これが「Fiddler 経由でだけ成功した」理由。

この事実自体が、**「問題は Codex ↔ gateway 間の直接の TLS にある」** ことの強力な傍証だった（プロキシが TLS を取り次ぐだけで直る＝TLS 実装の差異が原因）。

## 6. 解決策

### 6.1 ワークアラウンド（コード変更不要・即効）

`SSL_CERT_FILE` または `CODEX_CA_CERTIFICATE` に CA バンドルを設定すると、`custom_ca.rs` がカスタム CA ありパスに入り `use_rustls_tls()` で rustls を強制、Schannel を回避できる。

```cmd
curl.exe -o C:\Users\sysadmin\cacert.pem https://curl.se/ca/cacert.pem
set SSL_CERT_FILE=C:\Users\sysadmin\cacert.pem
codex mcp login employee-information
codex
```

**検証済み**: ログイン成功、認証後の tool call も成功。

### 6.2 恒久対策（コード修正案）

`codex-rs/http-client/src/custom_ca.rs` の `build_reqwest_client_with_env` で、カスタム CA がない場合（現在の `:350-368`）でも `use_rustls_tls()` を呼び、**常に rustls を使う**ようにする。これで Windows で Schannel を回避し、プラットフォーム間で TLS 挙動を統一できる。

## 7. 調査で否定した仮説（迷走の記録）

1. ❌ **DCR（Dynamic Client Registration）非対応** — simple-mcp-client は DCR 設定でも成功
2. ❌ **GET vs POST** — どちらも gateway は 401 を返す
3. ❌ **TLS 指紋（JA3）** — rustls テストプログラムで 401 取得、指紋説を否定
4. ❌ **rustls 自体の不具合** — rustls は健全（テストプログラムで証明）
5. ❌ **Accept ヘッダー順序** — どの順序でも 401
6. ❌ **Cookie store / W3C traceparent / stream** — いずれも 401

決定打は `RUST_LOG` に `codex_exec_server=debug` を追加して `log_send_error` の `error_sources` を見たこと。ここから Schannel の `SEC_E_ILLEGAL_MESSAGE` が浮上した。

## 8. エビデンス

### 8.1 決定ログ（RUST_LOG=codex_exec_server=debug）

```
connected to 52.43.88.253:443
WARN codex_exec_server::client::http_client::reqwest_http_client:
  http/request send failed http_method="GET"
  error_is_timeout=false error_is_connect=true
  error_sources=Some("client error (Connect): The message received was unexpected or badly formatted. (os error -2146893018)")
```

### 8.2 検証プログラム（simple-rust-check）

`reqwest 0.12` + `default-features=false` + `rustls-tls-native-roots` で rustls を強制した最小プログラムは、直接接続で GET / POST initialize とも **401** を取得（cookie store / W3C traceparent / Accept 順序を変えてもすべて 401）。Codex だけが失敗することを確認し、TLS バックエンドの差に帰着した。

### 8.3 curl での gateway 応答（直接接続）

```
HTTP/1.1 401 Unauthorized
Content-Length: 0
Connection: keep-alive
www-authenticate: Bearer resource_metadata="https://gateway.../.well-known/oauth-protected-resource/..."
```

gateway は正常。Schannel 以外の TLS バックエンドなら到達可能。

### 8.4 SSL_CERT_FILE ワークアラウンドの結果

```cmd
set SSL_CERT_FILE=C:\Users\sysadmin\cacert.pem
codex mcp login employee-information
→ Successfully logged in to MCP server 'employee-information'.
codex
→ employee-information が initialize 成功、tool call 成功
```

rustls 強制で Schannel を回避すると成功。根本原因の裏付け完了。

---

## 結論

Windows 版 Codex の MCP HTTP パスが **Schannel（native-tls）を使うことが原因**。`SSL_CERT_FILE` で rustls を強制すれば回避可能。恒久対策は Codex 側で `custom_ca.rs` を修正し常に rustls を使うようにすること。

証明書の Issued by は `IdenTrust / HydrantID Server CA O1`（公開CA）なので、証明書自体に問題はなく、純粋に Schannel の TLS 実装と gateway の非互換。
