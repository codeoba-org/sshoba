use clap::Parser;
use shhoba::cli::Cli;
use shhoba::core::paths::Paths;

fn init_logging(paths: &Paths) {
    use tracing_subscriber::EnvFilter;
    if std::fs::create_dir_all(&paths.state_dir).is_err() {
        return;
    }
    let Ok(file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(paths.log_file())
    else {
        return;
    };
    let filter = EnvFilter::try_from_env("SHHOBA_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(false)
        .with_writer(std::sync::Mutex::new(file))
        .try_init();
}

fn main() {
    let cli = Cli::parse();
    let paths = Paths::resolve(cli.config.as_deref());
    init_logging(&paths);
    tracing::debug!(?cli, "starting");
}
