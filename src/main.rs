use std::str::FromStr;

use clap::Parser;
use embedded_runner as _;

#[tokio::main]
async fn main() {
    let log_level =
        log::LevelFilter::from_str(&std::env::var("DEFMT_LOG").unwrap_or("info".to_string()))
            .unwrap_or(log::LevelFilter::Info);
    env_logger::Builder::from_default_env()
        .format_module_path(true)
        .filter_level(log_level)
        .init();

    let cfg = embedded_runner::cfg::CliConfig::parse();

    if let Err(err) = embedded_runner::run(cfg).await {
        log::error!("Embedded runner failed: {err}");
        std::process::exit(1);
    }
}
