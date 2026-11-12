//! End-to-end: a mock Anthropic upstream, the router in front of it, and a real HTTP client.
use axum::{routing::post, Json, Router};
use llm_router::{server, Thresholds};
use std::time::Duration;

async fn spawn(app: Router) -> String {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    format!("http://{addr}")
}

async fn mock_upstream() -> String {
    spawn(Router::new().route(
        "/v1/messages",
        post(|Json(v): Json<serde_json::Value>| async move {
            Json(serde_json::json!({"id":"msg_1","type":"message","role":"assistant","model": v["model"],
                "content":[{"type":"text","text":"ok"}],"usage":{"input_tokens": 1000, "output_tokens": 500}}))
        }),
    ))
    .await
}

fn cfg(upstream: String, budget: f64) -> server::Config {
    server::Config { upstream, thresholds: Thresholds::default(), daily_budget_usd: budget, cache_ttl: Duration::from_secs(60), cache_size: 100 }
}

#[tokio::test]
async fn routes_caches_and_accounts() {
    let router = spawn(server::app(cfg(mock_upstream().await, 0.0))).await;
    let c = reqwest::Client::new();
    let easy = serde_json::json!({"model":"auto","max_tokens":100,"messages":[{"role":"user","content":"What is the capital of France?"}]});
    let r = c.post(format!("{router}/v1/messages")).header("x-api-key", "test").json(&easy).send().await.unwrap();
    assert_eq!(r.headers()["x-router-model"], "claude-haiku-5-5");
    assert_eq!(r.headers()["x-router-cache"], "miss");
    let body: serde_json::Value = r.json().await.unwrap();
    assert_eq!(body["model"], "claude-haiku-5-5");

    let r2 = c.post(format!("{router}/v1/messages")).json(&easy).send().await.unwrap();
    assert_eq!(r2.headers()["x-router-cache"], "hit");

    let hard = serde_json::json!({"model":"auto","max_tokens":100,"messages":[{"role":"user","content":
        "Prove that the eigenvalues of a Hermitian matrix are real, derive the result step by step, then analyze the trade-offs of three numerical methods and justify which one scales best.\n1. state\n2. prove\n3. compare"}]});
    let r3 = c.post(format!("{router}/v1/messages")).json(&hard).send().await.unwrap();
    assert_eq!(r3.headers()["x-router-model"], "claude-opus-5-5");

    let stats: serde_json::Value = c.get(format!("{router}/stats")).send().await.unwrap().json().await.unwrap();
    assert_eq!(stats["requests"], 3);
    assert_eq!(stats["cache_hits"], 1);
    // haiku 1000 in + 500 out, then opus 1000 in + 500 out
    let expected = (1000.0 * 0.10 + 500.0 * 0.50 + 1000.0 * 4.0 + 500.0 * 20.0) / 1e6;
    assert!((stats["spent_usd"].as_f64().unwrap() - expected).abs() < 1e-12);
}

#[tokio::test]
async fn budget_downgrades_then_blocks() {
    let router = spawn(server::app(cfg(mock_upstream().await, 0.015))).await;
    let c = reqwest::Client::new();
    let hard = |i: u32| serde_json::json!({"model":"auto","max_tokens":10,"messages":[{"role":"user","content": format!(
        "Request {i}: prove the theorem, derive every step, analyze trade-offs, justify the design and optimize it.\n1. a\n2. b\n3. c")}]});
    let first = c.post(format!("{router}/v1/messages")).json(&hard(1)).send().await.unwrap();
    assert_eq!(first.headers()["x-router-model"], "claude-opus-5-5"); // costs $0.014: above 80% of the $0.015 budget
    let second = c.post(format!("{router}/v1/messages")).json(&hard(2)).send().await.unwrap();
    assert_eq!(second.headers()["x-router-model"], "claude-sonnet-5-5");
    let third = c.post(format!("{router}/v1/messages")).json(&hard(3)).send().await.unwrap();
    assert_eq!(third.status(), 402);
}

#[tokio::test]
async fn explicit_model_is_respected_and_stream_is_refused() {
    let router = spawn(server::app(cfg(mock_upstream().await, 0.0))).await;
    let c = reqwest::Client::new();
    let r = c.post(format!("{router}/v1/messages"))
        .json(&serde_json::json!({"model":"claude-opus-5-5","max_tokens":10,"messages":[{"role":"user","content":"hi"}]}))
        .send().await.unwrap();
    assert_eq!(r.headers()["x-router-model"], "claude-opus-5-5");
    let s = c.post(format!("{router}/v1/messages"))
        .json(&serde_json::json!({"model":"auto","stream":true,"messages":[{"role":"user","content":"hi"}]}))
        .send().await.unwrap();
    assert_eq!(s.status(), 400);
}
