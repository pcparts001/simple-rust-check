//! Cisco MCP-GW に対して Codex CLI と同一の rustls スタックでリクエストを送り、
//! 直接接続で失敗するのが rustls の TLS 指紋 (JA3) に起因するかを検証する。
//!
//! 判定:
//! - 両方のテストが `OK: status=401` → rustls でも gateway は 401 を返す → 別要因
//! - いずれかが `ERR: ...` (timeout 等)   → rustls の TLS 指紋が原因で確定

use std::time::Duration;

const URL: &str = "https://gateway.agent.preview.aidefense.aiteam.cisco.com/mcp/tenant/192caeea-9955-44a9-8ef4-19006f5beb10/connections/11559df8-3d9d-4658-ab2e-87abd16a1f6b/server/470747a9-4805-4d70-97c1-6077227ed802";

#[tokio::main]
async fn main() {
    // reqwest のデフォルトビルダー。features = ["rustls-tls-native-roots"] により
    // rustls + aws-lc-rs が使われる (Codex の MCP HTTP パスと同一)。
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .expect("failed to build reqwest client");

    println!("=== TEST 1: GET (Codex discovery 相当) ===");
    match client.get(URL).header("MCP-Protocol-Version", "2024-11-05").send().await {
        Ok(r) => println!(
            "OK: status={}, content-type={:?}, transfer-encoding={:?}",
            r.status(),
            header(&r, "content-type"),
            header(&r, "transfer-encoding"),
        ),
        Err(e) => println!("ERR: {e:?}"),
    }

    println!();
    println!("=== TEST 2: POST initialize (Codex MCP 通信 相当) ===");
    let body = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t","version":"1.0.0"}}}"#;
    match client
        .post(URL)
        .header("MCP-Protocol-Version", "2024-11-05")
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .body(body)
        .send()
        .await
    {
        Ok(r) => println!(
            "OK: status={}, content-type={:?}, transfer-encoding={:?}",
            r.status(),
            header(&r, "content-type"),
            header(&r, "transfer-encoding"),
        ),
        Err(e) => println!("ERR: {e:?}"),
    }
}

fn header<'a>(r: &'a reqwest::Response, name: &str) -> Option<&'a str> {
    r.headers().get(name).and_then(|v| v.to_str().ok())
}
