//! The HTTP proxy: an Anthropic Messages API endpoint that routes, caches, forwards and accounts.
use crate::{classify, cost_usd, last_user_text, Budget, Cache, Thresholds, Tier};
use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

#[derive(Clone, Debug)]
pub struct Config {
    pub upstream: String,
    pub thresholds: Thresholds,
    pub daily_budget_usd: f64,
    pub cache_ttl: Duration,
    pub cache_size: usize,
}

#[derive(Default, Serialize, Clone)]
pub struct Stats {
    pub requests: u64,
    pub cache_hits: u64,
    pub per_tier: HashMap<String, u64>,
    pub spent_usd: f64,
    pub opus_equivalent_usd: f64,
}

pub struct AppState {
    cfg: Config,
    http: reqwest::Client,
    cache: Mutex<Cache>,
    budget: Mutex<Budget>,
    stats: Mutex<Stats>,
}

pub fn app(cfg: Config) -> Router {
    let state = Arc::new(AppState {
        http: reqwest::Client::builder().timeout(Duration::from_secs(600)).build().unwrap(),
        cache: Mutex::new(Cache::new(cfg.cache_ttl, cfg.cache_size)),
        budget: Mutex::new(Budget { daily_usd: cfg.daily_budget_usd, spent_usd: 0.0, soft_fraction: 0.8 }),
        stats: Mutex::new(Stats::default()),
        cfg,
    });
    Router::new().route("/v1/messages", post(messages)).route("/stats", get(stats)).with_state(state)
}

async fn stats(State(s): State<Arc<AppState>>) -> Json<Stats> {
    Json(s.stats.lock().await.clone())
}

fn err(code: StatusCode, msg: &str) -> Response {
    (code, Json(serde_json::json!({"type":"error","error":{"type":"router_error","message":msg}}))).into_response()
}

async fn messages(State(s): State<Arc<AppState>>, headers: HeaderMap, body: Bytes) -> Response {
    let Ok(mut req) = serde_json::from_slice::<serde_json::Value>(&body) else {
        return err(StatusCode::BAD_REQUEST, "body is not JSON");
    };
    if req.get("stream").and_then(|v| v.as_bool()) == Some(true) {
        return err(StatusCode::BAD_REQUEST, "streaming is not supported by this router yet");
    }
    let asked = req.get("model").and_then(|m| m.as_str()).unwrap_or("auto").to_string();
    let (tier, score) = if asked == "auto" {
        let (t, sc, _) = classify(&last_user_text(&req), s.cfg.thresholds);
        let b = s.budget.lock().await;
        if b.exhausted() {
            return err(StatusCode::PAYMENT_REQUIRED, "daily budget exhausted");
        }
        (b.apply(t), sc)
    } else {
        let t = Tier::all().into_iter().find(|t| t.model() == asked).unwrap_or(Tier::Sonnet);
        (t, f64::NAN)
    };
    req["model"] = serde_json::Value::String(if asked == "auto" { tier.model().to_string() } else { asked.clone() });
    let final_body = serde_json::to_vec(&req).unwrap();
    let key = Cache::key(&final_body);

    {
        let mut st = s.stats.lock().await;
        st.requests += 1;
        *st.per_tier.entry(format!("{:?}", tier).to_lowercase()).or_default() += 1;
    }
    if let Some(hit) = s.cache.lock().await.get(&key) {
        s.stats.lock().await.cache_hits += 1;
        return respond(hit, tier, score, 0.0, true);
    }

    let mut up = s.http.post(format!("{}/v1/messages", s.cfg.upstream.trim_end_matches('/'))).header("content-type", "application/json");
    for h in ["x-api-key", "anthropic-version", "anthropic-beta", "authorization"] {
        if let Some(v) = headers.get(h) {
            up = up.header(h, v);
        }
    }
    let resp = match up.body(final_body).send().await {
        Ok(r) => r,
        Err(e) => return err(StatusCode::BAD_GATEWAY, &format!("upstream unreachable: {e}")),
    };
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return (StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY), text).into_response();
    }
    let v: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
    let it = v["usage"]["input_tokens"].as_u64().unwrap_or(0);
    let ot = v["usage"]["output_tokens"].as_u64().unwrap_or(0);
    let cost = cost_usd(tier, it, ot);
    {
        s.budget.lock().await.spent_usd += cost;
        let mut st = s.stats.lock().await;
        st.spent_usd += cost;
        st.opus_equivalent_usd += cost_usd(Tier::Opus, it, ot);
    }
    eprintln!("{}", serde_json::json!({"model": tier.model(), "score": score, "input_tokens": it, "output_tokens": ot, "cost_usd": cost}));
    s.cache.lock().await.put(key, text.clone());
    respond(text, tier, score, cost, false)
}

fn respond(body: String, tier: Tier, score: f64, cost: f64, cached: bool) -> Response {
    let mut r = (StatusCode::OK, body).into_response();
    let h = r.headers_mut();
    h.insert("content-type", HeaderValue::from_static("application/json"));
    h.insert("x-router-model", HeaderValue::from_static(tier.model()));
    h.insert("x-router-cache", HeaderValue::from_static(if cached { "hit" } else { "miss" }));
    if score.is_finite() {
        h.insert("x-router-score", HeaderValue::from_str(&format!("{score:.2}")).unwrap());
    }
    h.insert("x-router-cost-usd", HeaderValue::from_str(&format!("{cost:.6}")).unwrap());
    r
}
