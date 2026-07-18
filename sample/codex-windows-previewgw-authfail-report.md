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

## 考察: 誰が直すべきか / なぜ Schannel だけが失敗するか

### 「Ciscoが Schannel 対応するべき」ではない

curl も Windows の Schannel を使うが成功している。つまり gateway は **Schannel 自体を拒否しているのではなく、Codex が使う `schannel` crate（Rust から Schannel SSPI を叩く実装）とのみ非互換**。正確には「gateway と `schannel` crate の TLS ネゴシエーションが互換でない」。

### Cisco が意図的に拒否している可能性は低い

TLS ハンドシェイクの段階で失敗（`SEC_E_ILLEGAL_MESSAGE`）しているのは、**プロトコルレベルの純粋な非互換**の特徴。サーバーがクライアントを識別して意図的に拒否する（ボット対策等）なら、ふつう TLS を完遂させてから HTTP 層（403 等）で弾く。TLS メッセージをわざと壊して拒否するのは不自然。意図的というより純粋な実装差。

### 「セキュリティ的に古い」のは schannel crate 側

gateway は AWS 上（IP が AWS）で、AWS API Gateway / CloudFront 系と推定。AWS はセキュリティポリシーで **モダンな TLS（TLS 1.2/1.3、強い暗号スイース）のみ許可** する。実際 gateway は TLS 1.3（`TLS13_AES_128_GCM_SHA256`）を使う。

一方 `schannel` crate は:

- TLS 1.3 サポートが Windows バージョン依存（本格対応は Windows 11 / Server 2022 以降）
- ここ数年メンテナンスが停滞気味（rustls は超活発）
- 古い暗号/拡張を提示してネゴシエーション破談、または TLS 1.3 を扱えず失敗、の可能性

つまり「古い TLS 実装（schannel crate）がモダンな gateway に弾かれた」形。「セキュリティ的に古いから」という直感は、**古いのは gateway ではなく schannel crate / Windows 側** として部分的に正しい。

### 責任の所在と推奨

| 立場 | 評価 |
|---|---|
| Cisco（gateway） | rustls / OpenSSL / Secure Transport / curl-Schannel と広く互換。標準 TLS 実装済み。不具合とは言えない |
| `schannel` crate | TLS 1.3 未対応・バグの可能性。ただし第三者 crate で Codex は待てない |
| **Codex（OpenAI）** | **rustls 統一が最も実効的**。クロスプラットフォーム挙動統一・モダン TLS・活発なメンテ |

設計論としても、Codex が「カスタム CA 未設定時だけ native-tls にフォールバック」するのは **プラットフォーム間で TLS 挙動が割れる原因** になり好ましくない。常に rustls が筋。

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

### 8.5 ClientHello 実物（Wireshark キャプチャ・Codex 0.144.1 直接接続）

Codex（schannel 0.1.28）が gateway に送る ClientHello を Wireshark でキャプチャ:

```
TLSv1.2 Record Layer: Handshake Protocol: Client Hello
  Version: TLS 1.2 (0x0303)                         ← TLS 1.2 のみ
  Cipher Suites (18 suites) — すべて TLS 1.2:
    TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384 (0xC02C)
    TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256 (0xC02B)
    TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384  (0xC030)
    TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256  (0xC02F)
    ...（計18個、すべて TLS 1.2。TLS 1.3 の 0x1301〜0x1303 は一切なし）
    ※ TLS_RSA_WITH_AES_*（非PFS・弱い）も含む
  Extensions:
    server_name (= gateway.agent.preview.aidefense.aiteam.cisco.com)
    supported_groups, ec_point_formats, signature_algorithms,
    session_ticket, extended_master_secret, renegotiation_info
    ★ supported_versions(43) なし / key_share(51) なし
JA4: t12d180700_4b22cbed5bed_2dae41c691ec   ← 「t12」=TLS1.2, 「d」=ALPNなし
JA3: 771,49196-...-47,0-10-11-13-35-23-65281,...   ← 拡張に 43(supported_versions) なし
```

**決定的ポイント（ご指摘の「TLSスイートの違い」が完全に実証された）:**
1. ClientHello Version = TLS 1.2（0x0303）— TLS 1.3 を示さない
2. `supported_versions` 拡張が存在しない — TLS 1.3 を提示する拡張そのものがない
3. `key_share` 拡張がない — TLS 1.3 の鍵共有もない
4. 暗号スイートは TLS 1.2 のみ 18個（TLS 1.3 の `TLS_AES_*` が一切ない）
5. RSA 鍵交換の非PFSスイート（`TLS_RSA_WITH_AES_*`）を含む

→ **Codex（schannel 0.1.28）は TLS 1.3 を完全に無効化した状態で ClientHello を送る。** gateway は AWS ALB（TLS 1.3 / モダンTLS要求）。この TLS 1.3 非提示が `SEC_E_ILLEGAL_MESSAGE` の根本原因。

curl / rustls / Mac の Secure Transport はいずれも TLS 1.3 を提示するため成功。

---

## 結論

Windows 版 Codex の MCP HTTP パスが **Schannel（native-tls）を使うことが原因**。`SSL_CERT_FILE` で rustls を強制すれば回避可能。恒久対策は Codex 側で `custom_ca.rs` を修正し常に rustls を使うようにすること。

証明書の Issued by は `IdenTrust / HydrantID Server CA O1`（公開CA）なので、証明書自体に問題はなく、純粋に Schannel の TLS 実装と gateway の非互換。
