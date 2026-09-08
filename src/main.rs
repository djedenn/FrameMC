use std::sync::Arc;
use tokio::signal;
use tokio::sync::watch;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

use framemc::config::ProxyConfig;
use framemc::network::listener::start_listener;

/// Parses CLI arguments, returning the path to the configuration file,
/// or `None` if the program should exit immediately (e.g. after `--help`).
fn parse_args() -> Result<Option<String>, Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let mut config_path = "config.toml".to_string();
    let mut i = 1;

    while i < args.len() {
        match args[i].as_str() {
            "-c" | "--config" => {
                if i + 1 < args.len() {
                    config_path = args[i + 1].clone();
                    i += 2;
                } else {
                    eprintln!("Error: --config requires a path argument");
                    std::process::exit(1);
                }
            }
            "-h" | "--help" => {
                println!("FrameMC - High-Performance Minecraft Proxy");
                println!();
                println!("Usage: framemc [OPTIONS]");
                println!();
                println!("Options:");
                println!(
                    "  -c, --config <PATH>  Path to configuration file [default: config.toml]"
                );
                println!("  -h, --help           Display this help message");
                return Ok(None);
            }
            other => {
                eprintln!("Error: Unrecognized option '{other}'");
                eprintln!("Try '--help' for more information.");
                std::process::exit(1);
            }
        }
    }

    Ok(Some(config_path))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config_path = match parse_args()? {
        Some(path) => path,
        None => return Ok(()),
    };

    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .with(tracing_subscriber::fmt::layer())
        .init();

    tracing::info!(
        "Starting FrameMC Minecraft Proxy using config: {}",
        config_path
    );

    let config = ProxyConfig::load_or_create(&config_path)?;
    let config = Arc::new(config);

    // Initialize Rhai script host and register backend servers for scripting / plugins
    let script_host = Arc::new(framemc::script::engine::ScriptHost::new());
    script_host.set_servers(
        config.servers.keys().cloned().collect(),
        config.default_server.clone(),
    );

    // Load plugins from plugins directory
    match script_host.load_plugins_dir(&config.plugins_dir).await {
        Ok(count) => tracing::info!("Loaded {count} plugin(s) from '{}'", config.plugins_dir),
        Err(e) => tracing::warn!("Failed loading plugins from '{}': {e}", config.plugins_dir),
    }

    // Load main script if configured and present
    if std::path::Path::new(&config.script_path).exists() {
        if let Err(e) = script_host.reload(&config.script_path).await {
            tracing::warn!("Failed loading script at '{}': {e}", config.script_path);
        } else {
            tracing::info!("Loaded script at '{}'", config.script_path);
        }
    }

    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let shutdown_tx_clone = shutdown_tx.clone();
    tokio::spawn(async move {
        if let Ok(()) = signal::ctrl_c().await {
            tracing::info!(
                "OS shutdown signal (Ctrl-C) received, initiating graceful termination..."
            );
            let _ = shutdown_tx_clone.send(true);
        }
    });

    start_listener(config, script_host, shutdown_rx).await?;

    tracing::info!("FrameMC Proxy shutdown complete. Exiting cleanly.");
    Ok(())
}
