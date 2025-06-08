use anyhow::{Context, Result};
use clap::Parser;
use log::info;

use crate::actions;
use crate::git_ops::AvRepo; // Not directly used here, but good for consistency if needed later
use crate::meta::JsonFileDb;
use crate::{GIT_REPO, GLOBAL_CONFIG, gh::GhClient}; // Added GhClient import

#[derive(Parser, Debug)]
#[clap(about = "Clean up branches whose corresponding Pull Requests have been merged or closed.")]
pub struct TidyCliOpts {
    #[clap(long, help = "Perform a dry run: show what would be done, but don't do it.")]
    pub dry_run: bool,

    #[clap(long, help = "Specify the remote to use for fetching and deleting remote branches (defaults to 'origin' or configured default).")]
    pub remote: Option<String>,
}

pub async fn run_tidy_cmd(opts: TidyCliOpts) -> Result<()> {
    let av_repo = GIT_REPO.get().unwrap().as_ref()
        .context("Tidy command requires to be run inside a Git repository")?;
    let config = GLOBAL_CONFIG.get().expect("GLOBAL_CONFIG not initialized. This is a bug.");
    let db = JsonFileDb::new(&av_repo.common_dir);

    let repo_meta = db.read_state()?.repository
        .with_context(|| "Repository metadata not found. Please run `av init` first.")?;

    let remote_name_to_use = opts.remote.as_deref()
        .or(config.remote.as_deref())
        .unwrap_or("origin");

    let gh_token = config.github.token.as_deref()
        .context("GitHub token not configured. Please set AV_GITHUB_TOKEN or GITHUB_TOKEN, or configure in av.toml")?;
    let gh_client = GhClient::new(gh_token, config.github.base_url.as_deref())?;


    let action_opts = actions::tidy::TidyOpts {
        dry_run: opts.dry_run,
        remote_name: remote_name_to_use,
    };

    let tidied_branches = actions::tidy::tidy_branches(
        av_repo,
        &gh_client,
        &db,
        &repo_meta,
        action_opts
    ).await?;

    if tidied_branches.is_empty() {
        info!("No branches were tidied.");
    } else {
        info!("Successfully tidied the following branches:");
        for branch_name in tidied_branches {
            info!("  - {}", branch_name);
        }
    }
    Ok(())
}
