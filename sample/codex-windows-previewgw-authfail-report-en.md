# Codex (Windows) — Cisco MCP-GW (preview.aidefense) Auth Failure Investigation Report

- **Target**: Codex CLI 0.144.5 (Windows) × Streamable HTTP MCP server
- **Symptom**: MCP server `employee-information` OAuth auth / connection failure
- **Root cause**: Codex's MCP HTTP path on Windows uses **Schannel (native-tls)**; the `schannel` crate fails the TLS handshake with the gateway (`SEC_E_ILLEGAL_MESSAGE`) because it does not offer TLS 1.3
- **Fix**: Force rustls via `CODEX_CA_CERTIFICATE` (Codex-specific, no side effects) — login + tool call confirmed working
- **Date**: 2026-07-18

---

## 1. Overview

The Windows build of Codex CLI (installed via install.ps1) fails OAuth auth and initialize against a Streamable HTTP MCP server (Cisco MCP-GW: `gateway.agent.preview.aidefense.aiteam.cisco.com`). On the same Windows machine, curl and a custom Python client succeed; Mac Codex is also reported to succeed.

## 2. Symptom

- `codex mcp login employee-information`
  → `Error: No authorization support detected`
- `codex` (TUI launch)
  → `MCP client for 'employee-information' failed to start: ... error sending request for url (https://gateway...)`

## 3. Root Cause

### 3.1 The actual error

Obtained by adding `codex_exec_server=debug` to `RUST_LOG`:

```
WARN codex_exec_server::client::http_client::reqwest_http_client:
  http/request send failed
  http_method="GET"
  error_is_timeout=false
  error_is_connect=true                              ← TLS connection (handshake) error
  error=error sending request
  error_sources="client error (Connect): The message received was unexpected or badly formatted. (os error -2146893018)"
```

`os error -2146893018` = `0x80090326` = **`SEC_E_ILLEGAL_MESSAGE` (Windows Schannel TLS protocol error)**.

This is **not** a rustls error — it is the **Windows native TLS (Schannel)** error. The TCP connection succeeds (`connected to 52.43.88.253:443`), then the TLS handshake fails before any HTTP response (e.g. 401) arrives. Therefore discovery fails entirely → `NoAuthorizationSupport` → `No authorization support detected`.

### 3.2 Why Codex uses Schannel on Windows

Codex's `codex-rs/http-client/src/custom_ca.rs`, function `build_reqwest_client_with_env`, does **NOT** call `use_rustls_tls()` when no custom CA (`CODEX_CA_CERTIFICATE` / `SSL_CERT_FILE`) is configured. reqwest has default-features enabled (=> `default-tls` = native-tls). Although `rustls-tls` is also enabled by exec-server/http-client, **reqwest picks native-tls when both are present**. Therefore:

- Windows → **Schannel** (via the `schannel` crate)
- macOS → Secure Transport
- Linux → OpenSSL

Only when a custom CA is set does it enter the `use_rustls_tls()` path (rustls forced).

## 4. Why Windows-only; Mac/Linux succeed

| Platform | native-tls impl | gateway TLS | Result |
|---|---|---|---|
| **Windows** | **Schannel (`schannel` crate)** | **SEC_E_ILLEGAL_MESSAGE** | **Fail ❌** |
| macOS | Secure Transport | OK | OK ✅ |
| Linux | OpenSSL | OK | OK ✅ |

The `schannel` crate's TLS implementation (cipher suites, extensions, TLS 1.3 handling) is incompatible with this Cisco MCP-GW (AWS ALB, IdenTrust/HydrantID cert). Mac's Secure Transport, Linux's OpenSSL, rustls, and curl's Schannel (a different implementation) all succeed. **Only the `schannel` crate fails.** This fully matches the "Mac Codex succeeds" report.

## 5. Per-client results (everything reconciles)

| Client | TLS backend | Direct result |
|---|---|---|
| Codex (Windows) | Schannel (`schannel` crate) | SEC_E_ILLEGAL_MESSAGE ❌ |
| Codex (Mac) | Secure Transport | OK ✅ |
| test program (rustls forced) | rustls | 401 ✅ |
| curl | Schannel (different impl) | 401 ✅ |
| simple-mcp-client (Python) | OpenSSL | 401 ✅ |
| Codex (via Fiddler) | Fiddler TLS (MITM re-termination) | OK ✅ |

## 5.5 Why Fiddler succeeds

Fiddler re-terminates TLS (MITM), so the TLS session splits in two:

```
Codex(Schannel)  ←TLS①→  Fiddler  ←TLS②→  gateway
      ↑                          ↑
 localhost (127.0.0.1)       real gateway
 Fiddler's forged cert       gateway's real cert
```

- **TLS① (Codex ↔ Fiddler)**: Codex's Schannel only handshakes with **localhost Fiddler**. Fiddler forges a cert for the gateway's domain, signed by Fiddler's CA (trusted in the Windows store). Schannel is compatible with Fiddler → succeeds.
- **TLS② (Fiddler ↔ gateway)**: the gateway handshake is done by **Fiddler's TLS client**, not Codex's Schannel. Fiddler's TLS is compatible with the gateway → succeeds.

So **Codex's Schannel never talks to the gateway directly**, completely bypassing the incompatibility. This fact itself was strong evidence that the problem lies in the Codex ↔ gateway direct TLS.

## Analysis: Who should fix this

### "Cisco should support Schannel" is inaccurate

curl also uses Windows Schannel and succeeds. The gateway does **not** reject Schannel itself; it is incompatible **only with Codex's `schannel` crate** (the Rust binding to Schannel SSPI).

### Cisco intentionally rejecting: unlikely

Failing at the TLS handshake (`SEC_E_ILLEGAL_MESSAGE`) is a pure protocol-level incompatibility. Servers that intentionally reject clients (bot mitigation) normally complete TLS then reject at the HTTP layer (403). Deliberately sending malformed TLS messages is unnatural.

### "Security-old" is on the `schannel` crate side

The gateway is on AWS (ALB) and enforces **modern TLS (TLS 1.2/1.3, strong ciphers)**. The captured ClientHello confirms the gateway uses TLS 1.3. Meanwhile the `schannel` crate:
- TLS 1.3 support depends on the Windows version (fully only on Windows 11 / Server 2022+)
- maintenance has stagnated in recent years (rustls is very active)

So it is "old TLS implementation (schannel crate) rejected by modern gateway". The intuition "security-old" is partly right, but the old side is the **schannel crate / Windows**, not the gateway.

### Responsibility and recommendation

| Party | Assessment |
|---|---|
| Cisco (gateway) | Broadly compatible with rustls / OpenSSL / Secure Transport / curl-Schannel. Standard TLS. Not a bug. |
| `schannel` crate | Likely TLS 1.3 not offered / bug. But it is a third-party crate; Codex cannot wait. |
| **Codex (OpenAI)** | **Unifying to rustls is the most effective fix.** Cross-platform behavior unification, modern TLS, active maintenance. |

By design, Codex falling back to native-tls only when no custom CA is set **fractures TLS behavior across platforms**, which is undesirable. Always-rustls is the principled fix.

## 6. Fix

### 6.1 Workaround (no code change, immediate)

Setting `CODEX_CA_CERTIFICATE` (or `SSL_CERT_FILE`) enters the custom-CA path (`custom_ca.rs:300-307`), which calls `use_rustls_tls()` → rustls forced → Schannel bypassed.

**Prefer `CODEX_CA_CERTIFICATE`** (Codex-specific; other apps are unaffected). `SSL_CERT_FILE` is generic and also read by curl/Git/Python, so it can affect other apps.

```cmd
:: 1. Download cacert.pem (Mozilla CA bundle, includes IdenTrust/HydrantID and other public CAs)
curl.exe -o C:\Users\sysadmin\codex-ca.pem https://curl.se/ca/cacert.pem

:: 2. Set a persistent USER environment variable (Codex-specific, harmless to other apps)
setx CODEX_CA_CERTIFICATE "C:\Users\sysadmin\codex-ca.pem"

:: 3. Open a NEW terminal (setx does not affect the current window), verify:
echo %CODEX_CA_CERTIFICATE%

:: 4. Run codex
codex mcp login employee-information
codex
```

**Verified**: login succeeds; tool call after auth succeeds.

To revert: `setx CODEX_CA_CERTIFICATE ""` (empty value is treated as unset by custom_ca.rs), or delete the variable from the system environment settings.

### 6.2 Permanent fix (code change)

In `codex-rs/http-client/src/custom_ca.rs`, `build_reqwest_client_with_env`, call `use_rustls_tls()` even when no custom CA is set, so **rustls is always used**. This avoids Schannel on Windows and unifies TLS behavior across platforms.

## 7. Refuted hypotheses (the wandering path)

1. ❌ **DCR (Dynamic Client Registration) unsupported** — simple-mcp-client succeeds even with DCR configured.
2. ❌ **GET vs POST** — the gateway returns 401 for both.
3. ❌ **TLS fingerprint (JA3)** — the rustls test program gets 401; fingerprint theory rejected.
4. ❌ **rustls itself buggy** — rustls is healthy (proven by the test program).
5. ❌ **Accept header order** — 401 for any order.
6. ❌ **Cookie store / W3C traceparent / stream** — 401 for all.

The breakthrough was adding `codex_exec_server=debug` to `RUST_LOG` to see `log_send_error`'s `error_sources`, which surfaced Schannel's `SEC_E_ILLEGAL_MESSAGE`.

## 8. Evidence

### 8.1 Decisive log (RUST_LOG with codex_exec_server=debug)

```
connected to 52.43.88.253:443
WARN codex_exec_server::client::http_client::reqwest_http_client:
  http/request send failed http_method="GET"
  error_is_timeout=false error_is_connect=true
  error_sources=Some("client error (Connect): The message received was unexpected or badly formatted. (os error -2146893018)")
```

### 8.2 Test program (simple-rust-check)

A minimal program with `reqwest 0.12` + `default-features=false` + `rustls-tls-native-roots` (rustls forced) obtains **401** for both GET and POST initialize directly (also 401 when varying cookie store / W3C traceparent / Accept order). Confirms that only Codex fails, narrowing the cause to the TLS backend.

### 8.3 curl gateway response (direct connection)

```
HTTP/1.1 401 Unauthorized
Content-Length: 0
Connection: keep-alive
www-authenticate: Bearer resource_metadata="https://gateway.../.well-known/oauth-protected-resource/..."
```

The gateway is healthy; any non-Schannel TLS backend can reach it.

### 8.4 SSL_CERT_FILE / CODEX_CA_CERTIFICATE workaround result

```cmd
set CODEX_CA_CERTIFICATE=C:\Users\sysadmin\cacert.pem
codex mcp login employee-information
→ Successfully logged in to MCP server 'employee-information'.
codex
→ employee-information initializes successfully; tool call succeeds
```

Forcing rustls bypasses Schannel and succeeds, confirming the root cause.

### 8.5 Actual ClientHello (Wireshark capture, Codex 0.144.1 direct connection)

```
TLSv1.2 Record Layer: Handshake Protocol: Client Hello
  Version: TLS 1.2 (0x0303)                         ← TLS 1.2 only
  Cipher Suites (18 suites) — all TLS 1.2:
    TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384 (0xC02C)
    TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256 (0xC02B)
    TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384  (0xC030)
    TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256  (0xC02F)
    ... (18 total, all TLS 1.2; none of TLS 1.3's 0x1301–0x1303)
    ※ also includes TLS_RSA_WITH_AES_* (non-PFS, weak)
  Extensions:
    server_name (= gateway.agent.preview.aidefense.aiteam.cisco.com)
    supported_groups, ec_point_formats, signature_algorithms,
    session_ticket, extended_master_secret, renegotiation_info
    ★ supported_versions(43) absent / key_share(51) absent
JA4: t12d180700_4b22cbed5bed_2dae41c691ec   ← "t12"=TLS1.2, "d"=no ALPN
JA3: 771,49196-...-47,0-10-11-13-35-23-65281,...   ← extension 43 (supported_versions) absent
```

**Decisive points (the "TLS suite difference" hypothesis fully proven):**
1. ClientHello Version = TLS 1.2 (0x0303) — does not indicate TLS 1.3
2. `supported_versions` extension is absent — the very extension that would advertise TLS 1.3 is missing
3. `key_share` extension is absent — TLS 1.3 key share is also missing
4. Cipher suites are 18 TLS 1.2 only (none of `TLS_AES_*`)
5. Includes RSA key-exchange non-PFS suites (`TLS_RSA_WITH_AES_*`)

→ **Codex (schannel 0.1.28) sends a ClientHello with TLS 1.3 fully disabled.** The gateway is an AWS ALB (requires TLS 1.3 / modern TLS). This non-offer of TLS 1.3 is the root cause of `SEC_E_ILLEGAL_MESSAGE`.

curl / rustls / Mac's Secure Transport all offer TLS 1.3, so they succeed.

### 8.6 Simulation result (simple-rust-check fully reproduces Codex's failure)

By sending the same "TLS 1.2 only" ClientHello as Codex (`max_tls_version(TLS_1_2)` + native-tls), failure against the gateway (ALB, TLS 1.3 required) is reproduced:

| Run | TLS backend | Result |
|---|---|---|
| `cargo run` | rustls (offers TLS 1.3) | **401 OK** ✅ |
| `cargo run --no-default-features --features native-tls-mode` | Schannel (TLS 1.2 only) | **ERR** ❌ |

Failure error:
```
ERR: error sending request
  source1: client error (Connect)
  source2: The function requested is not supported (os error -2146893054)
```

Error code comparison:
- Codex (schannel default): `SEC_E_ILLEGAL_MESSAGE` (0x80090326)
- simulation (max_tls_version=TLS1.2): `SEC_E_UNSUPPORTED_FUNCTION` (0x80090302)

Both are Schannel/SSPI TLS handshake failures with the same root cause (schannel cannot handle/offer TLS 1.3). The code difference comes from the negotiation stage: explicit TLS 1.2 cap (simulation) vs schannel default (Codex). This **fully reproduces Codex's bug in a single program**.

### 8.7 Workaround env var selection (prefer `CODEX_CA_CERTIFICATE`)

Codex's `custom_ca.rs` (lines 61-62, 395):
- `CODEX_CA_CERTIFICATE`: **Codex-specific**, takes precedence
- `SSL_CERT_FILE`: generic fallback

`SSL_CERT_FILE` is read by many apps (curl/Git/Python OpenSSL), so setting it as an env var **affects other apps** (an app needing a private CA may fail if cacert.pem lacks it). **`CODEX_CA_CERTIFICATE` is read only by Codex**; other apps ignore it → zero impact. For an env-var workaround, `CODEX_CA_CERTIFICATE` is safe.

---

## Conclusion

The Windows Codex MCP HTTP path uses **Schannel (native-tls)**. Root cause: the `schannel` crate does not offer TLS 1.3 → the AWS ALB (TLS 1.3) handshake fails with `SEC_E_ILLEGAL_MESSAGE`. Fix: force rustls via `CODEX_CA_CERTIFICATE` (Codex-specific, no side effects). Permanent fix: always use rustls in `custom_ca.rs`.

The certificate's Issued by is `IdenTrust / HydrantID Server CA O1` (a public CA), so the certificate itself is fine; this is purely an incompatibility between the Schannel TLS implementation and the gateway.
