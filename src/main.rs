//! simple-rust-check
//!
//! Cisco MCP-GW に対して「native-tls（Windowsでは Schannel）」と「rustls 強制」の
//! 2つの TLS バックエンドでリクエストを送り、Windows 版 Codex の認証エラー
//! （Schannel が gateway と TLS ハンドシェイクで失敗する SEC_E_ILLEGAL_MESSAGE）
//! を 1 プログラム内で再現する。
//!
//! 期待結果:
//! - Windows  : クライアントA（native-tls=Schannel）= ERR / クライアントB（rustls）= 401
//! - macOS/Linux: 両方 = 401（Schannel ではないため → Windows 固有の裏付け）

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
    // クライアントA: native-tls 強制（WindowsではSchannel）← Codex の失敗パターンを再現
    //   ※ reqwest 0.12.28 は Client::builder().build() のデフォルトで rustls を選ぶことがあるため、
    //   明示的に use_native_tls() で Schannel を強制する。
    let native = reqwest::Client::builder()
        .cookie_store(true)
        .use_native_tls()
        .timeout(Duration::from_secs(20))
        .build()
        .expect("native-tls client build");

    // クライアントB: rustls 強制 ← SSL_CERT_FILE ワークアラウンドと同じ
    let rustls = reqwest::Client::builder()
        .cookie_store(true)
        .use_rustls_tls()
        .timeout(Duration::from_secs(20))
        .build()
        .expect("rustls client build");

    println!("対象URL: {URL}\n");

    run("A-1 native-tls GET", &native, HttpMethod::Get).await;
    run("A-2 native-tls POST initialize", &native, HttpMethod::Post).await;
    println!();
    run("B-1 rustls GET", &rustls, HttpMethod::Get).await;
    run("B-2 rustls POST initialize", &rustls, HttpMethod::Post).await;

    println!();
    println!("=== 判定 ===");
    println!("Windows  : A-1/A-2 が ERR（SEC_E_ILLEGAL_MESSAGE）、B-1/B-2 が 401 → Codex のバグ再現");
    println!("macOS/Linux: すべて 401（Schannel ではない → Windows 固有の裏付け）");
}

async fn run(label: &str, client: &reqwest::Client, method: HttpMethod) {
    println!("--- TEST {label} ---");
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
            // ボディをストリーム読み（held-open SSE ストリームの検知）
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
            // source chain を表示（SEC_E_ILLEGAL_MESSAGE 等の根本原因が見える）
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
