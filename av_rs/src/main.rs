use anyhow::Context;
use clap::{Parser, Subcommand};
use env_logger;
// Remove direct git2 dependency if AvRepo encapsulates all needed functionality from main's perspective
// use git2;
use log::{debug, error, info, warn};
use once_cell::sync::OnceCell;
use std::path::PathBuf;

// Modules
mod commands;
pub mod config;
pub mod gh;
pub mod git_ops;
pub mod meta;

// Bring config types and functions into scope
use config::{load_config, AvConfig};
use config::user_state::{load_user_state, UserState};
use git_ops::AvRepo;
use crate::commands::branch::BranchOpts;
use crate::commands::stack::StackOpts;
use crate::commands::pr::PrOpts; // Added for Pr command

// Global static variables, initialized in main
pub static GLOBAL_CONFIG: OnceCell<AvConfig> = OnceCell::new();
pub static GIT_REPO: OnceCell<Option<AvRepo>> = OnceCell::new();
// pub static USER_STATE: OnceCell<UserState> = OnceCell::new(); // User state can remain a local variable for now


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
    #[clap(about = "Manage stacked branches")]
    Branch(BranchOpts),
    #[clap(about = "Manage stacks of branches")]
    Stack(StackOpts),
    #[clap(about = "Manage pull requests")]
    Pr(PrOpts),
    #[clap(about = "Print the version information")]
    Version {},
    #[clap(about = "Initialize the repository for Aviator CLI")]
    Init {},
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // Corrected the Commands::Branch match arm from the previous turn.
    // It should have been `Commands::Branch(opts) => ...`

    if cli.debug {
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("debug")).init();
        debug!("Debug logging enabled.");
    } else {
        env_logger::init();
    }

    if let Some(dir) = &cli.directory {
        info!("Repository directory specified: {}", dir);
    }

    // Discover the repository and initialize GIT_REPO static variable
    let av_repo_instance = AvRepo::discover(cli.directory.as_deref());
    // Log whether repo discovery succeeded or failed, but don't error out here,
    // as some commands might not need a repo. Individual commands can check GIT_REPO.
    match &av_repo_instance {
        Ok(repo) => debug!("AvRepo initialized: workdir={:?}, common_dir={:?}", repo.workdir, repo.common_dir),
        Err(e) => debug!("AvRepo discovery failed: {}. Some commands may not work.", e),
    }
    if GIT_REPO.set(av_repo_instance.ok()).is_err() {
        return Err(anyhow::anyhow!("Failed to set GIT_REPO static variable. This is a bug."));
    }

    // Determine repository path for configuration loading using GIT_REPO
    let repo_av_config_path: Option<PathBuf> = GIT_REPO.get().unwrap().as_ref().map(|repo| repo.common_dir.join("av"));

    if let Some(path) = &repo_av_config_path {
        debug!("Repository config path for av.toml: {:?}", path);
    } else {
        debug!("No repository found or using global config only for av.toml.");
    }

    let cfg = load_config(repo_av_config_path.as_deref())
        .context("Failed to load av.toml configuration")?;

    if GLOBAL_CONFIG.set(cfg).is_err() {
        return Err(anyhow::anyhow!("Failed to set GLOBAL_CONFIG as it was already set. This is a bug."));
    }
    debug!("Global configuration loaded: {:?}", GLOBAL_CONFIG.get().unwrap());

    // Load user state
    let user_state = load_user_state().context("Failed to load user state")?;
    debug!("Loaded user state: {:?}", user_state);

    match cli.command {
        Commands::Adopt {} => {
            info!("[COMMAND] Adopt");
            // TODO: Implement adopt command logic
        }
        Commands::Auth {} => {
            info!("[COMMAND] Auth");
            // TODO: Implement auth command logic
        }
        Commands::Branch(branch_opts) => { // Correctly use branch_opts
            commands::branch::run_branch_cmd(branch_opts).await?
        }
        Commands::Stack(stack_opts) => { // Correctly use stack_opts
            commands::stack::run_stack_cmd(stack_opts).await?
        }
        Commands::Pr(pr_opts) => { // Add Pr command handling
            commands::pr::run_pr_cmd(pr_opts).await?
        }
        Commands::Version {} => {
            println!("{}", env!("CARGO_PKG_VERSION"));
        }
        Commands::Init {} => {
            // Pass None for directory as AvRepo is now globally discovered if possible
            commands::init::run(None).await?
        }
    }

    Ok(())
}
