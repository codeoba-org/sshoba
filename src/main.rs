use clap::{CommandFactory, Parser};
use sshoba::cli::Cli;
use sshoba::core::paths::Paths;

fn init_logging(paths: &Paths) {
    use tracing_subscriber::EnvFilter;
    if sshoba::core::fsutil::private_dir(&paths.state_dir).is_err() {
        return;
    }
    let Ok(file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(paths.log_file())
    else {
        return;
    };
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = file.set_permissions(std::fs::Permissions::from_mode(0o600));
    }
    let filter = EnvFilter::try_from_env("SSHOBA_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(false)
        .with_writer(std::sync::Mutex::new(file))
        .try_init();
}

fn main() {
    clap_complete::CompleteEnv::with_factory(Cli::command).complete();
    let cli = Cli::parse();
    init_logging(&Paths::resolve(cli.config.as_deref()));
    std::process::exit(sshoba::cli::commands::run(cli));
}
