use veyron_sdk::proto::PluginManifest;
use veyron_sdk::VeyronClient;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut client = VeyronClient::connect("/tmp/ai-smoke-veyron.sock").await?;
    let ack = client
        .register_full("smoke-test-caller", "0.1.0", PluginManifest::default(), "")
        .await?;
    assert!(ack.accepted, "registration rejected: {}", ack.reject_reason);

    let params = serde_json::json!({
        "provider": "openai",
        "base_url": "http://localhost:11434/v1",
        "model": "deepseek-coder:1.3b",
        "api_key_env": "OLLAMA_API_KEY",
        "messages": [{"role": "user", "content": "hi"}],
        "max_tokens": 20
    });
    let resp = client
        .send_action("chat_completion", params.to_string().as_bytes(), 10_000)
        .await?;
    println!("status: {}", resp.status);
    println!("error: {}", resp.error);
    Ok(())
}
