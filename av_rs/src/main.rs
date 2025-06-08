use anyhow::Context;
use clap::{Parser, Subcommand};
use env_logger;
use git2;
use log::{debug, error, info, warn}; // error and warn may not be used yet, but good to have.
use once_cell::sync::OnceCell;
use std::path::PathBuf;

// Modules
mod commands;
pub mod config;

// Bring config types and functions into scope
use config::{load_config, AvConfig};
use config::user_state::{load_user_state, UserState}; // save_user_state will be used by commands

// Global static configuration, initialized in main
pub static GLOBAL_CONFIG: OnceCell<AvConfig> = OnceCell::new();
// User state could also be a global static if commands need frequent access,
// or loaded/passed as needed. For now, load it in main and it can be passed.
// pub static USER_STATE: OnceCell<UserState> = OnceCell::new();


#[derive(Parser)]
#[clap(name = "av", version = "0.1.0", about = "Aviator CLI in Rust")]
struct Cli {
    #[clap(subcommand)]
    command: Commands,

    #[clap(long, short, global = true, help = "Enable verbose debug logging")]
    debug: bool,

    #[clap(long = "repo", short = 'C', global = true, help = "Directory to use for git repository")]
    directory: Option<String>,
}

#[derive(Subcommand)]
enum Commands {
    #[clap(about = "Adopt a branch into av")]
    Adopt {},
    #[clap(about = "Authenticate with GitHub")]
    Auth {},
    #[clap(about = "Manage branches")]
    Branch {},
    #[clap(about = "Print the version information")]
    Version {},
    #[clap(about = "Initialize the repository for Aviator CLI")]
    Init {},
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    if cli.debug {
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("debug")).init();
        debug!("Debug logging enabled.");
    } else {
        env_logger::init();
    }

    if let Some(dir) = &cli.directory {
        info!("Repository directory specified: {}", dir);
    }

    // Determine repository path for configuration loading
    let root_path_for_discovery = cli
        .directory
        .as_deref()
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().expect("Failed to get current directory"));

    let discovered_repo = git2::Repository::discover(&root_path_for_discovery)
        .map_err(|e| anyhow::anyhow!("Failed to discover git repository from {:?}: {}", root_path_for_discovery, e));

    let repo_av_config_path: Option<PathBuf> = match &discovered_repo {
        Ok(repo) => {
            let common_dir = repo.commondir(); // This is typically the .git folder
            debug!("Found git repository with common dir: {:?}", common_dir);
            Some(common_dir.join("av")) // Config expected in .git/av/
        }
        Err(e) => {
            // It's not necessarily an error to not be in a git repo for all commands
            // or for initial config loading (global configs might still be relevant).
            debug!("Git repository discovery failed or not in a repo: {}. Will only load global configs.", e);
            None
        }
    };

    let cfg = load_config(repo_av_config_path.as_deref())
        .context("Failed to load av.toml configuration")?;

    if GLOBAL_CONFIG.set(cfg).is_err() {
        // This should ideally not happen if main is the only place setting it.
        return Err(anyhow::anyhow!("Failed to set GLOBAL_CONFIG as it was already set. This is a bug."));
    }
    debug!("Global configuration loaded: {:?}", GLOBAL_CONFIG.get().unwrap());

    // Load user state
    let user_state = load_user_state().context("Failed to load user state")?;
    debug!("Loaded user state: {:?}", user_state);
    // Example: How a command might save user state (not called here)
    // save_user_state(&user_state).context("Failed to save user state changes")?;

    // TODO: Initialize Git repository object more formally if needed globally
    // The `discovered_repo` can be used or passed to commands that need it.

    match cli.command {
        Commands::Adopt {} => {
            info!("[COMMAND] Adopt");
            // TODO: Implement adopt command logic
        }
        Commands::Auth {} => {
            info!("[COMMAND] Auth");
            // TODO: Implement auth command logic
        }
        Commands::Branch {} => {
            info!("[COMMAND] Branch");
            // TODO: Implement branch command logic
        }
        Commands::Version {} => {
            println!("{}", env!("CARGO_PKG_VERSION"));
        }
        Commands::Init {} => {
            commands::init::run(cli.directory).await?
        }
    }

    Ok(())
}
