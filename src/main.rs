use alertsift::config::Config;
use alertsift::server::{now_utc, router, App};
use alertsift::store::Store;
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Parser)]
#[command(name = "alertsift", version, about = "Decide which alerts deserve to wake a human.")]
struct Cli {
    #[arg(long, short, default_value = "alertsift.toml", global = true)]
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

/// Graceful shutdown on SIGINT (Ctrl-C) or SIGTERM (the signal a supervisor sends to stop a
/// service), so an in-flight webhook gets to finish rather than being dropped mid-write.
async fn shutdown_signal() {
    let ctrl_c = async { let _ = tokio::signal::ctrl_c().await; };
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
            let alerts = alertsift::mapping::extract(&source, spec, &std::fs::read(payload)?)?;
            let redactor = alertsift::redact::Redactor::new(&cfg.redact.patterns);
            let store = Store::memory();
            for a in alerts {
                println!("{a:#?}");
                let p = alertsift::pipeline::prepare(&store, &redactor, a, now_utc(), cfg.decision.repeat_window_secs());
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
                    alertsift::digest::tick(&digest_app, now_utc()).await;
                    tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                }
            });
            let listener = tokio::net::TcpListener::bind(&listen).await?;
            eprintln!("alertsift listening on {listen}, mode {:?}", app.cfg.decision.mode);
            axum::serve(listener, router(app)).with_graceful_shutdown(shutdown_signal()).await?;
        }
        Cmd::Report { since, json } => {
            let store = Store::open(&cfg.store.path)?;
            let r = alertsift::report::build(&store, now_utc() - alertsift::report::parse_since(&since)?, alertsift::report::JEV_USD_PER_MTOK,
                cfg.decision.repeat_window_secs());
            if json {
                println!("{}", serde_json::to_string_pretty(&r)?);
            } else {
                print!("{}", r.to_text());
            }
        }
        Cmd::Replay { since } => {
            let store = Store::open(&cfg.store.path)?;
            let app = App::new(cfg, store, now_utc)?;
            let changed = alertsift::report::replay(&app, now_utc() - alertsift::report::parse_since(&since)?).await;
            if changed.is_empty() { println!("no decision changes"); }
            for (seq, summary, old, new) in changed { println!("#{seq} {old} -> {new}  {summary}"); }
        }
    }
    Ok(())
}
