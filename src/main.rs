use std::sync::Arc;
use tokio::sync::watch;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

use framemc::config::ProxyConfig;
use framemc::network::listener::start_listener;

#[derive(Debug, PartialEq, Eq)]
pub enum CliAction {
    Run { config_path: String },
    Help,
    Version,
}

/// Parses CLI arguments into a structured `CliAction`.
pub fn parse_cli_args<I, T>(args: I) -> Result<CliAction, String>
where
    I: IntoIterator<Item = T>,
    T: AsRef<str>,
{
    let args: Vec<String> = args.into_iter().map(|s| s.as_ref().to_string()).collect();
    let mut config_path = "config.toml".to_string();
    let mut i = 1;

    while i < args.len() {
        match args[i].as_str() {
            "-c" | "--config" => {
                if i + 1 < args.len() {
                    config_path = args[i + 1].clone();
                    i += 2;
                } else {
                    return Err("Error: --config requires a path argument".to_string());
                }
            }
            "-v" | "-V" | "--version" => {
                return Ok(CliAction::Version);
            }
            "-h" | "--help" => {
                return Ok(CliAction::Help);
            }
            other => {
                return Err(format!(
                    "Error: Unrecognized option '{other}'. Try '--help' for more information."
                ));
            }
        }
    }

    Ok(CliAction::Run { config_path })
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config_path = match parse_cli_args(std::env::args()) {
        Ok(CliAction::Run { config_path }) => config_path,
        Ok(CliAction::Version) => {
            println!("FrameMC v{}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        Ok(CliAction::Help) => {
            println!("FrameMC - High-Performance Minecraft Proxy");
            println!();
            println!("Usage: framemc [OPTIONS]");
            println!();
            println!("Options:");
            println!("  -c, --config <PATH>  Path to configuration file [default: config.toml]");
            println!("  -V, --version        Display version information");
            println!("  -h, --help           Display this help message");
            return Ok(());
        }
        Err(err_msg) => {
            eprintln!("{err_msg}");
            std::process::exit(1);
        }
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
        wait_for_shutdown_signal().await;
        let _ = shutdown_tx_clone.send(true);
    });

    start_listener(config, script_host, shutdown_rx).await?;

    tracing::info!("FrameMC Proxy shutdown complete. Exiting cleanly.");
    Ok(())
}

/// Listens for OS shutdown signals across platforms.
///
/// - On Unix (Linux & macOS): Listens concurrently for `SIGINT` (interactive Ctrl-C)
///   and `SIGTERM` (sent by systemd, Docker, Kubernetes, and launchd).
/// - On Windows: Listens for `tokio::signal::ctrl_c()`.
/// - Fallback for other platforms: Listens for `tokio::signal::ctrl_c()`.
#[cfg(unix)]
async fn wait_for_shutdown_signal() {
    use tokio::signal::unix::{signal, SignalKind};

    let sigint = signal(SignalKind::interrupt());
    let sigterm = signal(SignalKind::terminate());

    match (sigint, sigterm) {
        (Ok(mut int), Ok(mut term)) => {
            tokio::select! {
                _ = int.recv() => {
                    tracing::info!(
                        "OS shutdown signal (SIGINT / Ctrl-C) received, initiating graceful termination..."
                    );
                }
                _ = term.recv() => {
                    tracing::info!(
                        "OS shutdown signal (SIGTERM) received, initiating graceful termination..."
                    );
                }
            }
        }
        (Ok(mut int), Err(e)) => {
            tracing::warn!("Failed to install SIGTERM handler ({e}), falling back to SIGINT only");
            int.recv().await;
            tracing::info!(
                "OS shutdown signal (SIGINT / Ctrl-C) received, initiating graceful termination..."
            );
        }
        (Err(e), Ok(mut term)) => {
            tracing::warn!("Failed to install SIGINT handler ({e}), falling back to SIGTERM only");
            term.recv().await;
            tracing::info!(
                "OS shutdown signal (SIGTERM) received, initiating graceful termination..."
            );
        }
        (Err(e1), Err(e2)) => {
            tracing::error!(
                "Failed to install both SIGINT ({e1}) and SIGTERM ({e2}) handlers; running without OS signal listener"
            );
            std::future::pending::<()>().await;
        }
    }
}

#[cfg(windows)]
async fn wait_for_shutdown_signal() {
    if let Err(e) = tokio::signal::ctrl_c().await {
        tracing::error!(
            "Failed to listen for Ctrl-C shutdown signal ({e}); proxy will run without console shutdown signal handler"
        );
        std::future::pending::<()>().await;
    } else {
        tracing::info!("OS shutdown signal (Ctrl-C) received, initiating graceful termination...");
    }
}

#[cfg(not(any(unix, windows)))]
async fn wait_for_shutdown_signal() {
    if let Err(e) = tokio::signal::ctrl_c().await {
        tracing::error!(
            "Failed to listen for shutdown signal ({e}); proxy will run without signal handler"
        );
        std::future::pending::<()>().await;
    } else {
        tracing::info!("OS shutdown signal received, initiating graceful termination...");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cli_args_default() {
        let args = ["framemc"];
        let action = parse_cli_args(args).unwrap();
        assert_eq!(
            action,
            CliAction::Run {
                config_path: "config.toml".to_string()
            }
        );
    }

    #[test]
    fn test_cli_args_config_flag() {
        let args = ["framemc", "-c", "custom_proxy.toml"];
        let action = parse_cli_args(args).unwrap();
        assert_eq!(
            action,
            CliAction::Run {
                config_path: "custom_proxy.toml".to_string()
            }
        );

        let args2 = ["framemc", "--config", "other.toml"];
        let action2 = parse_cli_args(args2).unwrap();
        assert_eq!(
            action2,
            CliAction::Run {
                config_path: "other.toml".to_string()
            }
        );
    }

    #[test]
    fn test_cli_args_version_and_help() {
        assert_eq!(
            parse_cli_args(["framemc", "-v"]).unwrap(),
            CliAction::Version
        );
        assert_eq!(
            parse_cli_args(["framemc", "-V"]).unwrap(),
            CliAction::Version
        );
        assert_eq!(
            parse_cli_args(["framemc", "--version"]).unwrap(),
            CliAction::Version
        );
        assert_eq!(parse_cli_args(["framemc", "-h"]).unwrap(), CliAction::Help);
        assert_eq!(
            parse_cli_args(["framemc", "--help"]).unwrap(),
            CliAction::Help
        );
    }

    #[test]
    fn test_cli_args_errors() {
        assert!(parse_cli_args(["framemc", "-c"]).is_err());
        assert!(parse_cli_args(["framemc", "--unknown-flag"]).is_err());
    }

    #[tokio::test]
    async fn test_shutdown_channel_broadcast() {
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        assert!(!*shutdown_rx.borrow());
        let _ = shutdown_tx.send(true);
        assert!(*shutdown_rx.borrow());
    }

    #[tokio::test]
    async fn test_wait_for_shutdown_signal_does_not_prematurely_trigger() {
        let result = tokio::time::timeout(
            std::time::Duration::from_millis(50),
            wait_for_shutdown_signal(),
        )
        .await;
        assert!(
            result.is_err(),
            "wait_for_shutdown_signal must not complete prematurely without receiving an actual signal"
        );
    }
}
