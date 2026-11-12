//! llm-router: pick the cheapest Claude model that can handle a request.
//!
//! The classifier is deliberately simple and transparent: a weighted sum of features you can read
//! and tune (length, code, math, reasoning verbs, multi-step structure). No model is called to route.

use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, VecDeque};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    Haiku,
    Sonnet,
    Opus,
}

impl Tier {
    pub fn model(self) -> &'static str {
        match self {
            Tier::Haiku => "claude-haiku-5-5",
            Tier::Sonnet => "claude-sonnet-5-5",
            Tier::Opus => "claude-opus-5-5",
        }
    }
    /// USD per million tokens (input, output). Source: platform.claude.com/docs/en/about-claude/pricing, 2026-11-09.
    pub fn price(self) -> (f64, f64) {
        match self {
            Tier::Haiku => (0.10, 0.50),
            Tier::Sonnet => (2.0, 10.0),
            Tier::Opus => (4.0, 20.0),
        }
    }
    pub fn cheaper(self) -> Tier {
        match self {
            Tier::Opus => Tier::Sonnet,
            _ => Tier::Haiku,
        }
    }
    pub fn all() -> [Tier; 3] {
        [Tier::Haiku, Tier::Sonnet, Tier::Opus]
    }
}

pub fn cost_usd(tier: Tier, input_tokens: u64, output_tokens: u64) -> f64 {
    let (i, o) = tier.price();
    (input_tokens as f64 * i + output_tokens as f64 * o) / 1e6
}

/// Rough token estimate without a tokenizer: about 4 characters per token for English prose.
pub fn approx_tokens(text: &str) -> u64 {
    ((text.chars().count() as f64) / 4.0).ceil() as u64
}

#[derive(Debug, Clone, Serialize)]
pub struct Features {
    pub tokens: u64,
    pub code: bool,
    pub math: bool,
    pub reasoning_terms: usize,
    pub task_terms: usize,
    pub steps: usize,
    pub questions: usize,
}

fn re(cell: &'static OnceLock<Regex>, pat: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pat).unwrap())
}

pub fn features(text: &str) -> Features {
    static CODE: OnceLock<Regex> = OnceLock::new();
    static MATH: OnceLock<Regex> = OnceLock::new();
    static REASON: OnceLock<Regex> = OnceLock::new();
    static STEPS: OnceLock<Regex> = OnceLock::new();
    static TASK: OnceLock<Regex> = OnceLock::new();
    let low = text.to_lowercase();
    Features {
        tokens: approx_tokens(text),
        code: re(&CODE, r"```|\bdef |\bfn |\bclass |\bimport |#include|=>|\breturn\b|\{\s*\n").is_match(text),
        math: re(&MATH, r"\\(frac|sum|int|partial|mathbb)|\b(prove|derive|theorem|lemma|integral|eigen|gradient)\b|[=<>]\s*\d+\s*[\^*/]").is_match(&low),
        reasoning_terms: re(&REASON, r"\b(why|trade-?offs?|design|architecture|analy[sz]e|compare|evaluate|optimi[sz]e|debug|plan|strategy|critique|justify|edge cases?|security|scal(e|able|ability)|concurren\w*)\b")
            .find_iter(&low)
            .count(),
        task_terms: re(&TASK, r"\b(explain|write|create|implement|draft|summari[sz]e|review|convert|refactor|tests?|function|script|query|component|class)\b").find_iter(&low).count(),
        steps: re(&STEPS, r"(?m)^\s*(\d+[.)]|[-*•])\s+").find_iter(text).count(),
        questions: text.matches('?').count(),
    }
}

/// Difficulty score in [0, ~10]. Thresholds below turn it into a tier.
pub fn score(f: &Features) -> f64 {
    let len = (f.tokens as f64).ln_1p() * 0.9; // 100 tokens -> 4.2, 1000 -> 6.2
    let code = if f.code { 1.6 } else { 0.0 };
    let math = if f.math { 2.2 } else { 0.0 };
    let reason = (f.reasoning_terms as f64).min(5.0) * 0.75;
    let steps = (f.steps as f64).min(6.0) * 0.25;
    let q = (f.questions.saturating_sub(1) as f64).min(3.0) * 0.3;
    let task = (f.task_terms as f64).min(3.0) * 0.6;
    len + code + math + reason + steps + q + task - 3.0
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Thresholds {
    pub sonnet: f64,
    pub opus: f64,
}

impl Default for Thresholds {
    fn default() -> Self {
        Thresholds { sonnet: 0.4, opus: 1.1 } // tuned on the calibration half of data/prompts.jsonl (scripts/evaluate.py)
    }
}

pub fn classify(text: &str, t: Thresholds) -> (Tier, f64, Features) {
    let f = features(text);
    let s = score(&f);
    let tier = if s >= t.opus {
        Tier::Opus
    } else if s >= t.sonnet {
        Tier::Sonnet
    } else {
        Tier::Haiku
    };
    (tier, s, f)
}

/// Daily budget: once spending passes the soft limit, every route is moved one tier down.
#[derive(Debug)]
pub struct Budget {
    pub daily_usd: f64,
    pub spent_usd: f64,
    pub soft_fraction: f64,
}

impl Budget {
    pub fn apply(&self, tier: Tier) -> Tier {
        if self.daily_usd > 0.0 && self.spent_usd >= self.daily_usd * self.soft_fraction {
            tier.cheaper()
        } else {
            tier
        }
    }
    pub fn exhausted(&self) -> bool {
        self.daily_usd > 0.0 && self.spent_usd >= self.daily_usd
    }
}

/// Exact-match response cache with a time-to-live and a size limit (oldest entry evicted first).
pub struct Cache {
    ttl: Duration,
    cap: usize,
    map: HashMap<String, (Instant, String)>,
    order: VecDeque<String>,
}

impl Cache {
    pub fn new(ttl: Duration, cap: usize) -> Self {
        Cache { ttl, cap, map: HashMap::new(), order: VecDeque::new() }
    }
    pub fn key(body: &[u8]) -> String {
        hex::encode(Sha256::digest(body))
    }
    pub fn get(&mut self, k: &str) -> Option<String> {
        match self.map.get(k) {
            Some((t, v)) if t.elapsed() < self.ttl => Some(v.clone()),
            Some(_) => {
                self.map.remove(k);
                None
            }
            None => None,
        }
    }
    pub fn put(&mut self, k: String, v: String) {
        if !self.map.contains_key(&k) {
            self.order.push_back(k.clone());
        }
        self.map.insert(k, (Instant::now(), v));
        while self.map.len() > self.cap {
            if let Some(old) = self.order.pop_front() {
                self.map.remove(&old);
            }
        }
    }
    pub fn len(&self) -> usize {
        self.map.len()
    }
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

/// Text of the last user message in an Anthropic Messages API request body.
pub fn last_user_text(body: &serde_json::Value) -> String {
    let msgs = body.get("messages").and_then(|m| m.as_array());
    let Some(msgs) = msgs else { return String::new() };
    for m in msgs.iter().rev() {
        if m.get("role").and_then(|r| r.as_str()) != Some("user") {
            continue;
        }
        return match m.get("content") {
            Some(serde_json::Value::String(s)) => s.clone(),
            Some(serde_json::Value::Array(parts)) => parts
                .iter()
                .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join("\n"),
            _ => String::new(),
        };
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_factual_goes_to_haiku() {
        assert_eq!(classify("What is the capital of Iraq?", Thresholds::default()).0, Tier::Haiku);
    }

    #[test]
    fn proof_goes_to_opus() {
        let p = "Prove that the capacity of an AWGN channel is C = B log2(1 + SNR). Derive it step by step, \
                 justify each assumption, and analyze the trade-offs between bandwidth and power. \
                 1. state the model\n2. derive mutual information\n3. optimize the input distribution";
        assert_eq!(classify(p, Thresholds::default()).0, Tier::Opus);
    }

    #[test]
    fn code_review_goes_at_least_to_sonnet() {
        let p = "Debug this function and explain why it fails on edge cases:\n```python\ndef mean(x):\n    return sum(x)/len(x)\n```";
        assert!(classify(p, Thresholds::default()).0 >= Tier::Sonnet);
    }

    #[test]
    fn budget_moves_one_tier_down() {
        let b = Budget { daily_usd: 10.0, spent_usd: 9.0, soft_fraction: 0.8 };
        assert_eq!(b.apply(Tier::Opus), Tier::Sonnet);
        assert_eq!(b.apply(Tier::Haiku), Tier::Haiku);
        assert!(!b.exhausted());
    }

    #[test]
    fn cost_matches_price_table() {
        assert!((cost_usd(Tier::Sonnet, 1_000_000, 1_000_000) - 12.0).abs() < 1e-9);
        assert!((cost_usd(Tier::Haiku, 2_000, 500) - 0.00045).abs() < 1e-12);
    }

    #[test]
    fn cache_ttl_and_capacity() {
        let mut c = Cache::new(Duration::from_millis(50), 2);
        c.put("a".into(), "1".into());
        c.put("b".into(), "2".into());
        c.put("c".into(), "3".into());
        assert_eq!(c.len(), 2);
        assert!(c.get("a").is_none());
        assert_eq!(c.get("c").as_deref(), Some("3"));
        std::thread::sleep(Duration::from_millis(60));
        assert!(c.get("c").is_none());
    }

    #[test]
    fn extracts_last_user_text_from_blocks() {
        let v: serde_json::Value = serde_json::json!({"messages":[{"role":"user","content":"old"},{"role":"assistant","content":"x"},
            {"role":"user","content":[{"type":"text","text":"hello"},{"type":"text","text":"world"}]}]});
        assert_eq!(last_user_text(&v), "hello\nworld");
    }
}
pub mod server;
