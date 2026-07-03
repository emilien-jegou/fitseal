use clap::Parser;
use fitseal::{process_instruction_text, run_daemon, Cli, Commands};
use std::collections::HashSet;
use std::fs;
use colored::Colorize;
use tracing::{info, error};

#[cfg(test)]
mod tests;

fn main() {
    let cli = Cli::parse();

    let _log_guard = if let Some(log_path) = &cli.log_file {
        let file_appender = tracing_appender::rolling::never("", log_path);
        let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);
        tracing_subscriber::fmt()
            .with_writer(non_blocking)
            .with_env_filter(tracing_subscriber::EnvFilter::new("debug"))
            .with_thread_ids(true)
            .init();
        Some(guard)
    } else {
        let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("error"));
        tracing_subscriber::fmt()
            .with_writer(std::io::stderr)
            .with_env_filter(env_filter)
            .init();
        None
    };

    match cli.command {
        Commands::Daemon { auto_apply } => run_daemon(auto_apply),
        Commands::Apply { file, dry_run, yes } => {
            info!("Running manual apply for file: {}", file);
            let content = fs::read_to_string(&file).unwrap_or_else(|e| {
                error!("Failed to read apply target {}: {}", file, e);
                eprintln!("{} Failed to read {}: {}", "✖ Error:".red().bold(), file, e);
                std::process::exit(1);
            });
            let mut cache = HashSet::new();
            if !process_instruction_text(&content, yes || dry_run, dry_run, &mut cache) {
                std::process::exit(1);
            }
        }
    }
}
