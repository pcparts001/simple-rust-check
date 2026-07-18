//! simple-rust-check
//!
//! feature で TLS バックエンドを切り替え、Windows版Codexの Schannel 非互換を再現する。
//!
//! 実行:
//! - `cargo run`                                                  → rustls（成功パターン、401）
//! - `cargo run --no-default-features --features native-tls-mode` → native-tls（WindowsではSchannel、失敗パターン）
//!
//! 期待結果（Windows）:
//! - rustls        → 401（成功）
//! - native-tls    → ERR（SEC_E_ILLEGAL_MESSAGE / os error -2146893018）← Codex のバグ再現

use std::time::Duration;

use futures_util::StreamExt;

const URL: &str = "https://gateway.agent.preview.aidefense.aiteam.cisco.com/mcp/tenant/192caeea-9955-44a9-8ef4-19006f5beb10/connections/11559df8-3d9d-4658-ab2e-87abd16a1f6b/server/470747a9-4805-4d70-97c1-6077227ed802";

const INITIALIZE_BODY: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t","version":"1.0.0"}}}"#;

enum HttpMethod {
    Get,
    Post,
}

#[tokio::main]
async fn main() {
    // feature で TLS バックエンドを表示（コンパイル時に決定）
    #[cfg(feature = "native-tls-mode")]
    let tls_name = "native-tls (WindowsではSchannel) ← Codex の失敗パターン";
    #[cfg(all(feature = "rustls-mode", not(feature = "native-tls-mode")))]
    let tls_name = "rustls ← 成功パターン";
    #[cfg(not(any(feature = "native-tls-mode", feature = "rustls-mode")))]
    let tls_name = "不明（feature 未指定）";

    // native-tls-mode のときは TLS 1.2 のみに制限（Codex の schannel 0.1.28 が
    // TLS 1.3 を提示しない挙動を再現）。rustls-mode は TLS 1.3 を提示（成功側）。
    #[cfg(feature = "native-tls-mode")]
    let client = reqwest::Client::builder()
        .cookie_store(true)
        .max_tls_version(reqwest::tls::Version::TLS_1_2)
        .timeout(Duration::from_secs(20))
        .build()
        .expect("client build");
    #[cfg(not(feature = "native-tls-mode"))]
    let client = reqwest::Client::builder()
        .cookie_store(true)
        .timeout(Duration::from_secs(20))
        .build()
        .expect("client build");

    println!("TLS backend: {tls_name}");
    println!("対象URL: {URL}\n");

    run("GET", &client, HttpMethod::Get).await;
    run("POST initialize", &client, HttpMethod::Post).await;
}

async fn run(label: &str, client: &reqwest::Client, method: HttpMethod) {
    println!("--- {label} ---");
    let req = match method {
        HttpMethod::Get => client.get(URL).header("MCP-Protocol-Version", "2024-11-05"),
        HttpMethod::Post => client
            .post(URL)
            .header("MCP-Protocol-Version", "2024-11-05")
            .header("Content-Type", "application/json")
            .header("Accept", "text/event-stream, application/json")
            .body(INITIALIZE_BODY),
    };
    match req.send().await {
        Ok(r) => {
            let status = r.status();
            let ct = header(&r, "content-type").map(String::from);
            let te = header(&r, "transfer-encoding").map(String::from);
            println!("OK: status={status}, content-type={ct:?}, transfer-encoding={te:?}");
            let mut stream = r.bytes_stream();
            let mut total = 0usize;
            let mut held_open = false;
            while let Some(chunk) = stream.next().await {
                match chunk {
                    Ok(c) => total += c.len(),
                    Err(e) => {
                        println!("stream read err after {total} bytes: {e}");
                        break;
                    }
                }
                if total > 1024 * 1024 {
                    held_open = true;
                    break;
                }
            }
            if held_open {
                println!("stream exceeded 1MiB — held-open SSE stream suspected");
            } else {
                println!("stream ended, total body bytes = {total}");
            }
        }
        Err(e) => {
            println!("ERR: {e}");
            let mut src = std::error::Error::source(&e);
            let mut depth = 1;
            while let Some(s) = src {
                println!("  source{depth}: {s}");
                src = s.source();
                depth += 1;
            }
        }
    }
    println!();
}

fn header<'a>(r: &'a reqwest::Response, name: &str) -> Option<&'a str> {
    r.headers().get(name).and_then(|v| v.to_str().ok())
}
