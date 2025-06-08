use anyhow::{Context, Result, anyhow};
use clap::{Args, Parser, Subcommand};
use log::{info, debug, warn};

use crate::actions; // Added for actions::pr module
use crate::git_ops::AvRepo;
use crate::meta::{JsonFileDb, BranchMeta, PullRequestMeta};
use crate::gh::GhClient;
use crate::{GIT_REPO, GLOBAL_CONFIG};

#[derive(Parser, Debug)]
pub struct PrOpts {
    #[clap(subcommand)]
    pub command: PrCommand
}

#[derive(Subcommand, Debug)]
pub enum PrCommand {
    Create(PrCreateOpts)
}

#[derive(Args, Debug)]
pub struct PrCreateOpts {
    #[clap(long, short)]
    pub title: Option<String>,
    #[clap(long, short)]
    pub body: Option<String>,
    #[clap(long)]
    pub draft: bool,
    #[clap(long, help="Do not push the branch before creating PR")]
    pub no_push: bool,
    // TODO: reviewers, base, force
}

pub async fn run_pr_cmd(opts: PrOpts) -> Result<()> {
    match opts.command {
        PrCommand::Create(create_opts) => handle_pr_create(create_opts).await
    }
}

async fn handle_pr_create(opts: PrCreateOpts) -> Result<()> {
    let av_repo = GIT_REPO.get().unwrap().as_ref().context("PR command requires Git repo")?;
    let config = GLOBAL_CONFIG.get().expect("GLOBAL_CONFIG not initialized");
    let gh_token = config.github.token.as_deref().context("GitHub token not configured (set GITHUB_TOKEN or AV_GITHUB_TOKEN, or configure in av.toml)")?;
    let gh_client = GhClient::new(gh_token, config.github.base_url.as_deref())?;

    let current_branch_name = av_repo.current_branch_name()?;
    let default_remote = config.remote.as_deref().unwrap_or("origin");

    if av_repo.is_trunk_branch(&current_branch_name, default_remote)? {
        return Err(anyhow!("Cannot create PR for a trunk branch: {}", current_branch_name));
    }

    let db = JsonFileDb::new(&av_repo.common_dir);
    let mut branch_meta = db.get_branch(&current_branch_name)?
        .with_context(|| format!("Branch '{}' not found in av metadata. Use 'av branch create' or ensure it's tracked.", current_branch_name))?;

    if branch_meta.pull_request.is_some() {
        // TODO: Add --force flag to allow re-creating / updating
        warn!("Branch '{}' already has an associated PR: #{}", current_branch_name, branch_meta.pull_request.as_ref().unwrap().number);
        info!("To update an existing PR, use 'av pr update' (not yet implemented) or use --force (not yet implemented).");
        // For now, let's allow overwriting the PR metadata if we proceed.
        // Or, more safely, exit here. Let's exit for now.
        return Err(anyhow!("Branch '{}' already has an associated PR: #{}. Use 'av pr update' or --force to modify.", current_branch_name, branch_meta.pull_request.unwrap().number));
    }

    let parent_branch_name = branch_meta.parent_branch.as_deref()
        .with_context(|| format!("Branch '{}' does not have a parent set in metadata. Cannot determine base for PR. Use 'av branch set-parent'.", current_branch_name))?;

    // Title: Use flag, or commit summary of topmost commit not on parent.
    let title = if let Some(t) = opts.title { t } else {
        let current_head_oid = av_repo.find_commit(&current_branch_name)?.context("Cannot find current branch head")?.id();
        let parent_head_oid = av_repo.find_commit(parent_branch_name)?.context(format!("Cannot find parent branch head for '{}'", parent_branch_name))?.id();

        let commits = av_repo.list_commits(current_head_oid, Some(parent_head_oid))
            .context("Failed to list commits for generating PR title")?;

        if let Some(top_commit_oid) = commits.first() {
            av_repo.get_commit_summary(*top_commit_oid)?
        } else {
            // This case means current branch head is same as parent branch head, or no unique commits.
            warn!("No unique commits found on branch '{}' compared to parent '{}'. Using branch name as title.", current_branch_name, parent_branch_name);
            format!("PR for {}", current_branch_name) // Fallback title
        }
    };
    debug!("Determined PR title: {}", title);

    let body = opts.body.unwrap_or_default();

    let repo_meta_state = db.read_state()?.repository
        .with_context(|| "Repository metadata not initialized. Please run `av init` to fetch repository details.")?;

    let action_opts = actions::pr::CreatePullRequestOpts {
        repo_owner: &repo_meta_state.owner.login, // Corrected: access login field
        repo_name: &repo_meta_state.name,
        repo_node_id: &repo_meta_state.id,
        head_ref_name: &current_branch_name,
        base_ref_name: parent_branch_name, // This is &str from as_deref
        title, // Already a String
        body,  // Already a String
        is_draft: opts.draft,
        no_push: opts.no_push,
        default_remote_name: default_remote,
    };

    match actions::pr::create_pull_request(av_repo, &gh_client, &db, action_opts).await {
        Ok(created_pr) => {
            info!("Successfully created PR #{} for branch '{}' via action: {}", created_pr.number, current_branch_name, created_pr.permalink);
            // TODO: Potentially open browser here if desired by config/flags
        }
        Err(e) => {
            return Err(e.context(format!("Failed to create PR for branch '{}'", current_branch_name)));
        }
    }

    // Note: Metadata update is now handled by the `actions::pr::create_pull_request` function.
    // The old direct metadata update code is removed from here.

    // TODO: Add reviewers (could be another action or part of CreatePullRequestOpts)
    // TODO: Update PR body with stack info (e.g. list of other PRs in the stack) - likely another action

    Ok(())
}
