use anyhow::{anyhow, Context, Result}; // Ensure anyhow is imported directly if using anyhow! macro
use clap::{Args, Subcommand, Parser};
use log::{info, debug};

use crate::git_ops::AvRepo;
use crate::meta::{JsonFileDb, BranchMeta}; // BranchMeta is from crate::meta, not crate::meta::types directly here
use crate::GIT_REPO; // Assuming GIT_REPO is pub static in main.rs or lib.rs

#[derive(Parser, Debug)]
pub struct BranchOpts {
    #[clap(subcommand)]
    pub command: BranchCommand,
}

#[derive(Subcommand, Debug)]
pub enum BranchCommand {
    /// Create a new stacked branch
    Create(CreateBranchOpts),
    // TODO: Add other subcommands like delete, list, rename, set-parent
}

#[derive(Args, Debug)]
pub struct CreateBranchOpts {
    /// Name of the new branch to create
    pub branch_name: String,
    /// Name of the parent branch (defaults to current branch)
    #[clap(short, long)]
    pub parent: Option<String>,
    // TODO: Add --draft, --no-push, etc. flags from Go version later
}

pub async fn run_branch_cmd(opts: BranchOpts) -> Result<()> {
    match opts.command {
        BranchCommand::Create(create_opts) => handle_create_branch(create_opts).await,
    }
}

async fn handle_create_branch(opts: CreateBranchOpts) -> Result<()> {
    info!("Creating new branch: {}", opts.branch_name);
    let av_repo = GIT_REPO.get().unwrap().as_ref()
        .context("Branch command requires to be run inside a Git repository")?;

    let parent_branch_name = match &opts.parent {
        Some(p_name) => {
            // Ensure parent branch exists
            if !av_repo.branch_exists(p_name, None)? { // None for local branch check
                return Err(anyhow!("Parent branch '{}' does not exist locally.", p_name));
            }
            p_name.clone()
        }
        None => av_repo.current_branch_name()
            .context("Failed to get current branch name to use as parent. Are you in detached HEAD?")?,
    };
    info!("Parent branch determined as: {}", parent_branch_name);

    // Get parent branch's head commit OID
    let parent_head_commit_obj = av_repo.find_commit(&parent_branch_name)?
        .with_context(|| format!("Could not find commit for parent branch '{}'", parent_branch_name))?;
    let parent_head_oid_str = parent_head_commit_obj.id().to_string();
    debug!("Parent branch '{}' head is at commit {}", parent_branch_name, parent_head_oid_str);

    // Create the new branch in Git, pointing to the parent's current HEAD
    // The Go version does `git checkout -b <new_branch_name> <parent_branch_name>`
    // This also switches to the new branch. Our `checkout_branch` does this.
    // The third argument to checkout_branch is new_head_ref_name. We want to base it on parent_branch_name's HEAD.
    av_repo.checkout_branch(&opts.branch_name, true, Some(&parent_branch_name))
        .with_context(|| format!("Failed to create and checkout git branch '{}' from '{}'", opts.branch_name, parent_branch_name))?;
    info!("Successfully created and checked out git branch '{}' from '{}'", opts.branch_name, parent_branch_name);

    // Get the new branch's head commit OID (should be same as parent_head_oid_str initially)
    let new_branch_head_commit_obj = av_repo.find_commit(&opts.branch_name)?
        .with_context(|| format!("Could not find commit for new branch '{}'", opts.branch_name))?;
    let new_branch_head_oid_str = new_branch_head_commit_obj.id().to_string();
    debug!("New branch '{}' head is at commit {}", opts.branch_name, new_branch_head_oid_str);

    // Create BranchMeta
    let branch_meta = BranchMeta {
        name: opts.branch_name.clone(),
        parent_branch: Some(parent_branch_name.clone()),
        parent_commit: Some(parent_head_oid_str), // This is the OID of parent when we branched off
        head_commit: new_branch_head_oid_str,   // This is the current OID of the new branch's head
        pull_request: None,
    };
    debug!("New branch metadata: {:?}", branch_meta);

    // Save metadata
    let db = JsonFileDb::new(&av_repo.common_dir);
    db.upsert_branch(&branch_meta)
        .context("Failed to save branch metadata")?;

    info!("Branch '{}' created successfully and metadata saved.", opts.branch_name);
    Ok(())
}
