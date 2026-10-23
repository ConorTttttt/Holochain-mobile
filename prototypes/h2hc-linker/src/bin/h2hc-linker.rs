//! h2hc-linker binary entry point

use clap::Parser;
use h2hc_linker::{Configuration, LinkerService};
use std::net::IpAddr;
use tracing_subscriber::{
    fmt::{self, format::FmtSpan, time::UtcTime},
    layer::SubscriberExt,
    EnvFilter, Registry,
};

const DEFAULT_LOG_LEVEL: &str = "info";

/// Command line arguments for h2hc-linker
#[derive(clap::Parser, Debug)]
#[command(name = "h2hc-linker")]
#[command(about = "Holochain-to-Holochain Linker - Network edge gateway for lightweight clients")]
pub struct Args {
    /// The address to bind to
    #[arg(short, long, env = "H2HC_LINKER_ADDRESS", default_value = "127.0.0.1")]
    pub address: IpAddr,

    /// The port to bind to
    #[arg(short, long, env = "H2HC_LINKER_PORT", default_value = "8090")]
    pub port: u16,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Install the default rustls crypto provider
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .expect("Failed to install rustls crypto provider");

    initialize_tracing()?;

    let args = Args::parse();
    let config = Configuration::from_env()?;

    tracing::info!(
        address = %args.address,
        port = %args.port,
        kitsune_enabled = config.kitsune_enabled(),
        registration_enabled = config.registration_enabled(),
        "Starting h2hc-linker"
    );

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    let service = LinkerService::new(args.address, args.port, config).await?;

    // Set up shutdown signal handler (ctrl-c + SIGTERM)
    let shutdown_task = tokio::spawn(async move {
        shutdown_signal().await;
        tracing::info!("Shutdown signal received");
        let _ = shutdown_tx.send(true);
    });

    service.run(shutdown_rx).await?;

    // Clean up the signal handler task
    shutdown_task.abort();

    Ok(())
}

/// Wait for either ctrl-c or SIGTERM (on Unix).
async fn shutdown_signal() {
    let ctrl_c = tokio::signal::ctrl_c();

    #[cfg(unix)]
    {
        let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler");
        tokio::select! {
            _ = ctrl_c => {}
            _ = sigterm.recv() => {}
        }
    }

    #[cfg(not(unix))]
    {
        ctrl_c.await.ok();
    }
}

fn initialize_tracing() -> Result<(), tracing::subscriber::SetGlobalDefaultError> {
    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(DEFAULT_LOG_LEVEL));

    let formatting_layer = fmt::layer()
        .with_timer(UtcTime::rfc_3339())
        .with_span_events(FmtSpan::NEW | FmtSpan::CLOSE)
        .with_file(true)
        .with_line_number(true);

    let subscriber = Registry::default().with(env_filter).with(formatting_layer);

    tracing::subscriber::set_global_default(subscriber)
}
