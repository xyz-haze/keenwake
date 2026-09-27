use clap::{Parser, Subcommand};
use keenwake::config::Config;
use keenwake::server::{now_utc, router, App};
use keenwake::store::Store;
use std::path::PathBuf;
use std::sync::Arc;

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
        #[arg(long, default_value = "7d")]
        since: String,
        #[arg(long)]
        json: bool,
    },
    /// Re-decide stored alerts with the current config (mapping is not replayed; repeat
    /// suppression is simulated from the replayed decisions). Calls the backend.
    Replay {
        #[arg(long, default_value = "7d")]
        since: String,
    },
}

/// How long the worker may keep deciding already-accepted webhooks after a stop signal.
const DRAIN: std::time::Duration = std::time::Duration::from_secs(8);

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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let cfg = Config::load(&cli.config)?;
    match cli.cmd {
        Cmd::CheckSource { source, payload } => {
            let spec = cfg.sources.get(&source).ok_or_else(|| anyhow::anyhow!("unknown source {source}"))?;
            let alerts = keenwake::mapping::extract(&source, spec, &std::fs::read(payload)?)?;
            let redactor = keenwake::redact::Redactor::new(&cfg.redact.patterns);
            let store = Store::memory();
            for a in alerts {
                println!("{a:#?}");
                let p = keenwake::pipeline::prepare(&store, &redactor, a, now_utc(), cfg.decision.repeat_window_secs());
                println!("state sent to the model:\n  {}\n", p.state);
            }
        }
        Cmd::Serve => {
            let store = Store::open(&cfg.store.path)?;
            let listen = cfg.server.listen.clone();
            let app = Arc::new(App::new(cfg, store, now_utc)?);
            let digest_app = app.clone();
            tokio::spawn(async move {
                loop {
                    keenwake::digest::guarded_tick(&digest_app, now_utc()).await;
                    tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                }
            });
            let (queue, worker) = keenwake::worker::start(app.clone(), keenwake::worker::QUEUE_CAPACITY);
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
            let store = Store::open(&cfg.store.path)?;
            let r = keenwake::report::build(
                &store,
                now_utc() - keenwake::report::parse_since(&since)?,
                keenwake::report::JEV_USD_PER_MTOK,
                cfg.decision.repeat_window_secs(),
            );
            if json {
                println!("{}", serde_json::to_string_pretty(&r)?);
            } else {
                print!("{}", r.to_text());
            }
        }
        Cmd::Replay { since } => {
            let store = Store::open(&cfg.store.path)?;
            let app = App::new(cfg, store, now_utc)?;
            let changed = keenwake::report::replay(&app, now_utc() - keenwake::report::parse_since(&since)?).await;
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
