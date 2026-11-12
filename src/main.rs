use clap::{Parser, Subcommand};
use llm_router::{classify, server, Thresholds};
use std::io::BufRead;
use std::time::{Duration, Instant};

#[derive(Parser)]
#[command(name = "llm-router", version, about = "Route each Claude request to Haiku, Sonnet or Opus by difficulty and budget")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the proxy. Point your Anthropic client's base URL at it and send model = "auto".
    Serve {
        #[arg(long, default_value = "127.0.0.1:8787")]
        listen: String,
        #[arg(long, default_value = "https://api.anthropic.com")]
        upstream: String,
        /// Daily budget in USD (0 = no limit). Above 80% of it, routes move one tier down.
        #[arg(long, default_value_t = 0.0)]
        daily_budget: f64,
        #[arg(long, default_value_t = 600)]
        cache_ttl_secs: u64,
        #[arg(long, default_value_t = 1000)]
        cache_size: usize,
        #[arg(long, default_value_t = 0.4)]
        sonnet_threshold: f64,
        #[arg(long, default_value_t = 1.1)]
        opus_threshold: f64,
    },
    /// Classify prompts from a JSONL file ({"prompt": ...}) and print one JSON decision per line.
    Route { file: String },
    /// Measure classifier throughput.
    Bench { file: String },
}

#[tokio::main]
async fn main() -> anyhow_free::Result {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Serve { listen, upstream, daily_budget, cache_ttl_secs, cache_size, sonnet_threshold, opus_threshold } => {
            let cfg = server::Config {
                upstream,
                thresholds: Thresholds { sonnet: sonnet_threshold, opus: opus_threshold },
                daily_budget_usd: daily_budget,
                cache_ttl: Duration::from_secs(cache_ttl_secs),
                cache_size,
            };
            let l = tokio::net::TcpListener::bind(&listen).await.expect("bind");
            eprintln!("llm-router listening on http://{listen}  (send model = \"auto\")");
            axum::serve(l, server::app(cfg)).await.expect("server");
        }
        Cmd::Route { file } => {
            for p in prompts(&file) {
                let (t, s, f) = classify(&p.1, Thresholds::default());
                println!("{}", serde_json::json!({"id": p.0, "tier": t, "score": (s * 100.0).round() / 100.0, "features": f}));
            }
        }
        Cmd::Bench { file } => {
            let ps = prompts(&file);
            let n = 200;
            let t0 = Instant::now();
            let mut sink = 0usize;
            for _ in 0..n {
                for p in &ps {
                    sink += classify(&p.1, Thresholds::default()).0 as usize;
                }
            }
            let dt = t0.elapsed().as_secs_f64();
            let total = (n * ps.len()) as f64;
            println!("{}", serde_json::json!({"prompts": total, "seconds": dt, "per_second": total / dt, "micros_per_prompt": dt * 1e6 / total, "sink": sink}));
        }
    }
    Ok(())
}

fn prompts(file: &str) -> Vec<(String, String)> {
    let f = std::fs::File::open(file).expect("open prompts file");
    std::io::BufReader::new(f)
        .lines()
        .map_while(Result::ok)
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let v: serde_json::Value = serde_json::from_str(&l).expect("valid JSON line");
            (v["id"].as_str().unwrap_or("").to_string(), v["prompt"].as_str().unwrap_or("").to_string())
        })
        .collect()
}

mod anyhow_free {
    pub type Result = std::result::Result<(), Box<dyn std::error::Error>>;
}
