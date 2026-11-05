//! PhoneLink daemon. Normally started by systemd: `systemctl --user enable --now phonelinkd`.

use phonelink_desktop::{
    config::Config,
    daemon::{run, DaemonOptions},
    paths,
};
use tokio::signal::unix::{signal, SignalKind};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,quinn=warn,mdns_sd=warn".into()),
        )
        .without_time() // journald adds timestamps
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stderr()))
        .init();

    if std::env::args().any(|a| a == "--version" || a == "-V") {
        println!("phonelinkd {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    let config_path = paths::config_file();
    let config = Config::load(&config_path)?;
    let opts = DaemonOptions {
        data_dir: paths::data_dir(),
        socket: paths::socket_path(),
        config,
        config_path: Some(config_path),
        popups: None,
    };
    let mut term = signal(SignalKind::terminate())?;
    run(opts, async move {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    })
    .await
}
