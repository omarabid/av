use anyhow::{anyhow, Context, Result};
use clap::{Args, Subcommand, Parser};
use log::{info, debug, warn}; // Added warn for rebase simulation

use crate::git_ops::AvRepo;
use crate::meta::{JsonFileDb, BranchMeta};
use crate::GIT_REPO;

#[derive(Parser, Debug)]
pub struct BranchOpts {
    #[clap(subcommand)]
    pub command: BranchCommand,
}

#[derive(Subcommand, Debug)]
pub enum BranchCommand {
    /// Create a new stacked branch
    Create(CreateBranchOpts),
    /// List stacked branches and their metadata
    List(ListBranchOpts),
    /// Delete a stacked branch and its metadata
    Delete(DeleteBranchOpts),
    /// Set or change the parent of a stacked branch
    SetParent(SetParentOpts),
}

#[derive(Args, Debug)]
pub struct CreateBranchOpts {
    /// Name of the new branch to create
    pub branch_name: String,
    /// Name of the parent branch (defaults to current branch)
    #[clap(short, long)]
    pub parent: Option<String>,
}

#[derive(Args, Debug)]
pub struct ListBranchOpts {
    // TODO: Flags like --all, --current-stack, --json
}

#[derive(Args, Debug)]
pub struct DeleteBranchOpts {
    /// Name of the branch to delete
    pub branch_name: String,
    #[clap(long, help = "Also delete the branch from the Git repository")]
    pub git: bool,
    // TODO: --force (for metadata if children exist), --force-git (for git branch -D)
}

#[derive(Args, Debug)]
pub struct SetParentOpts {
    /// The branch whose parent is to be changed
    pub child_branch: String,
    /// The new parent branch
    pub new_parent_branch: String,
    // TODO: --no-rebase flag?
}

pub async fn run_branch_cmd(opts: BranchOpts) -> Result<()> {
    match opts.command {
        BranchCommand::Create(create_opts) => handle_create_branch(create_opts).await,
        BranchCommand::List(list_opts) => handle_list_branches(list_opts).await,
        BranchCommand::Delete(delete_opts) => handle_delete_branch(delete_opts).await,
        BranchCommand::SetParent(set_parent_opts) => handle_set_parent_branch(set_parent_opts).await,
    }
}

async fn handle_list_branches(_opts: ListBranchOpts) -> Result<()> {
    let av_repo = GIT_REPO.get().unwrap().as_ref()
        .context("`av branch list` requires being inside a Git repository.")?;
    let db = JsonFileDb::new(&av_repo.common_dir);
    let branches = db.get_all_branches().context("Failed to load branch metadata")?;

    if branches.is_empty() {
        info!("No branches are currently tracked by av.");
        return Ok(());
    }

    info!("Tracked branches (av metadata):");
    // Simple list format for now. Could be improved with a table or tree structure.
    for (name, meta) in branches.iter().filter(|(n, _)| !n.is_empty()) { // Filter out empty name just in case
        println!("  - Branch: {}", name);
        if let Some(parent) = &meta.parent_branch {
            println!("    Parent: {} (at OID: {})", parent, meta.parent_commit.as_deref().unwrap_or("N/A"));
        } else {
            println!("    Parent: <none> (likely a trunk or adopted branch)");
        }
        println!("    HEAD OID: {}", meta.head_commit);
        if let Some(pr) = &meta.pull_request {
            println!("    PR #{}: {}", pr.number, pr.permalink);
        } else {
            println!("    PR: <none>");
        }
    }
    Ok(())
}

async fn handle_delete_branch(opts: DeleteBranchOpts) -> Result<()> {
    info!("Attempting to delete branch '{}'...", opts.branch_name);
    let av_repo = GIT_REPO.get().unwrap().as_ref()
        .context("`av branch delete` requires being inside a Git repository.")?;
    let db = JsonFileDb::new(&av_repo.common_dir);

    let branch_to_delete = db.get_branch(&opts.branch_name)?
        .with_context(|| format!("Branch '{}' not found in av metadata. Cannot delete.", opts.branch_name))?;

    // Check if other branches have this branch as a parent (safeguard)
    // TODO: Add a --force flag to bypass this check.
    let all_branches = db.get_all_branches()?;
    for (name, meta) in all_branches.iter() {
        if meta.parent_branch.as_deref() == Some(&opts.branch_name) {
            return Err(anyhow!(
                "Branch '{}' is a parent of '{}'. Please reparent or delete child branches first, or use --force (not yet implemented).",
                opts.branch_name, name
            ));
        }
    }

    if opts.git {
        info!("Deleting branch '{}' from Git repository...", opts.branch_name);
        let current_branch_opt = av_repo.current_branch_name().ok();
        if current_branch_opt.as_deref() == Some(&opts.branch_name) {
            let default_remote = crate::GLOBAL_CONFIG.get().unwrap().remote.as_deref().unwrap_or("origin");
            let parent_checkout_target = branch_to_delete.parent_branch.as_deref()
                .or_else(|| av_repo.default_branch_shorthand(default_remote).ok().as_deref());

            if let Some(checkout_target) = parent_checkout_target {
                info!("Current branch is '{}', checking out '{}' before deletion.", opts.branch_name, checkout_target);
                av_repo.checkout_branch(checkout_target, false, None)
                    .with_context(|| format!("Failed to checkout parent branch '{}'", checkout_target))?;
            } else {
                return Err(anyhow!(
                    "Cannot delete current branch '{}' without a known parent or default branch to switch to.",
                    opts.branch_name
                ));
            }
        }

        match av_repo.git2_repo.find_branch(&opts.branch_name, git2::BranchType::Local) {
            Ok(mut branch) => {
                branch.delete().with_context(|| format!("Failed to delete git branch '{}'", opts.branch_name))?;
                info!("Successfully deleted git branch '{}'.", opts.branch_name);
            },
            Err(e) if e.code() == git2::ErrorCode::NotFound => {
                info!("Git branch '{}' not found locally, no git deletion needed.", opts.branch_name);
            }
            Err(e) => return Err(e.into()).with_context(|| format!("Error finding git branch '{}' for deletion", opts.branch_name)),
        }
    }

    db.delete_branch(&opts.branch_name).context("Failed to delete branch from metadata")?;
    info!("Branch '{}' successfully deleted from av metadata.", opts.branch_name);
    Ok(())
}

async fn handle_set_parent_branch(opts: SetParentOpts) -> Result<()> {
    info!("Attempting to set parent of '{}' to '{}'...", opts.child_branch, opts.new_parent_branch);
    let av_repo = GIT_REPO.get().unwrap().as_ref()
        .context("`av branch set-parent` requires being inside a Git repository.")?;
    let db = JsonFileDb::new(&av_repo.common_dir);

    let mut child_meta = db.get_branch(&opts.child_branch)?
        .with_context(|| format!("Child branch '{}' not found in av metadata.", opts.child_branch))?;

    // Ensure the new parent branch exists in Git (could be in metadata or just a local/remote branch)
    let new_parent_commit_obj = av_repo.find_commit(&opts.new_parent_branch)?
        .with_context(|| format!("New parent branch/commit '{}' not found in Git.", opts.new_parent_branch))?;
    let new_parent_head_oid_str = new_parent_commit_obj.id().to_string();

    // --- Git Rebase Operation (Simplified as per plan) ---
    // TODO: Implement a more robust rebase, possibly by shelling out or more detailed git2 usage.
    // For now, we will update metadata and warn the user about manual rebase if needed.
    let current_branch_name = av_repo.current_branch_name().ok();
    let needs_rebase = child_meta.parent_commit.as_deref() != Some(new_parent_head_oid_str.as_str());

    if needs_rebase {
        warn!(
            "Branch '{}' needs to be rebased onto new parent '{}' ({}).",
            opts.child_branch, opts.new_parent_branch, new_parent_head_oid_str
        );
        warn!("Automatic rebase is not yet fully implemented in `av-rs`.");
        if current_branch_name.as_deref() == Some(opts.child_branch.as_str()) {
            warn!(
                "Please run: git rebase --onto {} {}",
                opts.new_parent_branch,
                child_meta.parent_branch.as_deref().unwrap_or(&child_meta.parent_commit.clone().unwrap_or_default()) // old_parent_ref
            );
        } else {
             warn!(
                "Please checkout '{}' and then run: git rebase --onto {} {}",
                opts.child_branch,
                opts.new_parent_branch,
                child_meta.parent_branch.as_deref().unwrap_or(&child_meta.parent_commit.clone().unwrap_or_default())
            );
        }
        // After manual rebase, the user would need to update head_commit in metadata.
        // For now, we update parent info and leave head_commit as is, which might be misleading if rebase occurs.
        // A better approach for future:
        // 1. If on child_branch, attempt rebase. If successful, update head_commit.
        // 2. If not on child_branch, or rebase fails, instruct user and don't update head_commit.
    }
    // --- End Git Rebase Operation (Simplified) ---

    child_meta.parent_branch = Some(opts.new_parent_branch.clone());
    child_meta.parent_commit = Some(new_parent_head_oid_str);
    // child_meta.head_commit should be updated if rebase happened and was successful.
    // For now, leaving it as is and relying on user to fix if they rebase.
    // If no rebase was needed, head_commit is still correct.

    db.upsert_branch(&child_meta).context("Failed to update child branch metadata after reparenting")?;
    info!(
        "Branch '{}' successfully reparented to '{}' in av metadata.",
        opts.child_branch, opts.new_parent_branch
    );
    if needs_rebase {
        info!("Remember to perform the git rebase operation as mentioned above if you haven't already.");
    }
    Ok(())
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
