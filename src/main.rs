use anyhow::Context;
use clap::{Parser, Subcommand};
use keenwake::config::Config;
use keenwake::mapping::{extract, unresolved};
use keenwake::pipeline::prepare;
use keenwake::redact::Redactor;
use keenwake::report::{self, parse_since};
use keenwake::server::{now_utc, router, App};
use keenwake::store::Store;
use keenwake::{digest, worker};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

#[derive(Parser)]
#[command(name = "keenwake", version, about = "Decide which alerts deserve to wake a human.")]
struct Cli {
    #[arg(long, short, default_value = "keenwake.toml", global = true)]
    config: PathBuf,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Receive webhooks on /hook/{source}.
    Serve,
    /// Show what a source mapping extracts from a payload, and the sentence sent to the model. No network.
    CheckSource {
        #[arg(long)]
        source: String,
        payload: PathBuf,
    },
    /// Summarise decisions over a window, e.g. --since 7d.
    Report {
        #[arg(long, default_value = "7d", value_parser = parse_since)]
        since: i64,
        #[arg(long)]
        json: bool,
    },
    /// Re-decide stored alerts with the current config (mapping is not replayed; repeat
    /// suppression is simulated from the replayed decisions). Calls the backend.
    Replay {
        #[arg(long, default_value = "7d", value_parser = parse_since)]
        since: i64,
    },
}

/// How long the worker may keep deciding already-accepted webhooks after a stop signal.
const DRAIN: Duration = Duration::from_secs(8);

/// Resolves on SIGINT (Ctrl-C) or SIGTERM (the signal a supervisor sends to stop a service).
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler")
            .recv()
            .await;
    };
    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}

/// Opens the store named by the config; sqlite's own error does not say which file.
fn open_store(cfg: &Config) -> anyhow::Result<Store> {
    Store::open(&cfg.store.path).with_context(|| format!("cannot open store {}", cfg.store.path))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let cfg = Config::load(&cli.config)?;
    match cli.cmd {
        Cmd::CheckSource { source, payload } => {
            let spec = cfg.sources.get(&source).ok_or_else(|| anyhow::anyhow!("unknown source {source}"))?;
            let body = std::fs::read(&payload).with_context(|| format!("cannot read payload {}", payload.display()))?;
            // Before extracting, so a missing required field is explained by the pointer that missed.
            let warnings = unresolved(spec, &body)?;
            for w in &warnings {
                eprintln!("warning: {w}");
            }
            let alerts = extract(&source, spec, &body)?;
            let redactor = Redactor::new(&cfg.redact.patterns);
            let store = Store::memory();
            for a in alerts {
                println!("{a:#?}");
                let p = prepare(&store, &redactor, a, now_utc(), cfg.decision.repeat_window_secs());
                println!("state sent to the model:\n  {}\n", p.state);
            }
            if !warnings.is_empty() {
                anyhow::bail!("{} field reference(s) resolved to nothing, see the warnings above", warnings.len());
            }
        }
        Cmd::Serve => {
            let store = open_store(&cfg)?;
            let listen = cfg.server.listen.clone();
            let app = Arc::new(App::new(cfg, store, now_utc)?);
            let digest_app = app.clone();
            tokio::spawn(async move {
                loop {
                    digest::guarded_tick(&digest_app, now_utc()).await;
                    tokio::time::sleep(Duration::from_secs(60)).await;
                }
            });
            let (queue, worker) = worker::start(app.clone(), worker::QUEUE_CAPACITY, worker::QUEUE_MAX_BYTES);
            let listener = tokio::net::TcpListener::bind(&listen).await?;
            eprintln!("keenwake listening on {listen}, mode {:?}", app.cfg.decision.mode);
            // On a stop signal, serve stops accepting and returns once open requests are answered;
            // the router, and with it the last queue sender, is then dropped. The worker decides
            // what was already accepted (at most DRAIN), then the process exits: anything still
            // queued after that is lost, and the source got a 200 for it.
            axum::serve(listener, router(app, queue)).with_graceful_shutdown(shutdown_signal()).await?;
            if tokio::time::timeout(DRAIN, worker).await.is_err() {
                eprintln!("keenwake: queue not drained within {DRAIN:?}, exiting anyway");
            }
        }
        Cmd::Report { since, json } => {
            let store = open_store(&cfg)?;
            let r = report::build(
                &store,
                now_utc() - since,
                report::list_price_per_mtok(&cfg.backend),
                cfg.decision.repeat_window_secs(),
            );
            if json {
                println!("{}", serde_json::to_string_pretty(&r)?);
            } else {
                print!("{r}");
            }
        }
        Cmd::Replay { since } => {
            let store = open_store(&cfg)?;
            let app = App::new(cfg, store, now_utc)?;
            let changed = report::replay(&app, now_utc() - since).await;
            if changed.is_empty() {
                println!("no decision changes");
            }
            for c in changed {
                println!("#{} {} -> {}  {}", c.seq, c.old.as_str(), c.new.as_str(), c.summary);
            }
        }
    }
    Ok(())
}
