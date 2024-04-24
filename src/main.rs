use clap::Parser;
use embedded_runner as _;

#[tokio::main]
async fn main() {
    env_logger::init();

    let cfg = embedded_runner::cfg::CliConfig::parse();

    if let Err(err) = embedded_runner::run(cfg).await {
        log::error!("Embedded runner failed: {err}");
        std::process::exit(1);
    }
}
