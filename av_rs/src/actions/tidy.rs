use anyhow::{Context, Result};
use log::{info, warn, debug};
use std::collections::HashSet;

use crate::git_ops::AvRepo;
use crate::gh::GhClient;
// Assuming PullRequestState is in gh::types, which is correct from previous tasks.
use crate::gh::types::PullRequestState as GhPullRequestState;
use crate::meta::{JsonFileDb, RepositoryMeta}; // RepositoryMeta is an alias for gh::GhRepositoryDetails

#[derive(Debug, Default)]
pub struct TidyOpts<'a> {
    pub dry_run: bool,
    pub remote_name: &'a str,
    // pub current_stack_only: bool, // Future enhancement
}

pub async fn tidy_branches(
    av_repo: &AvRepo,
    gh_client: &GhClient,
    db: &JsonFileDb<'_>,
    repo_meta: &RepositoryMeta, // This is crate::gh::GhRepositoryDetails
    opts: TidyOpts<'_>,
) -> Result<Vec<String>> {
    info!("Starting tidy operation (dry_run: {}, remote: '{}')", opts.dry_run, opts.remote_name);

    // 1. Fetch from remote with prune
    debug!("Fetching from remote '{}' with prune...", opts.remote_name);
    av_repo.fetch(opts.remote_name, &[], true) // true for prune
        .with_context(|| format!("Failed to fetch from remote '{}'", opts.remote_name))?;
    debug!("Fetch complete.");

    let all_local_meta_branches = db.get_all_branches()?;
    let mut tidied_branches = Vec::new();
    let mut branches_to_keep_for_parenting = HashSet::new();

    // Pre-calculate which branches are parents of other av-tracked branches
    for meta in all_local_meta_branches.values() {
        if let Some(parent_name) = &meta.parent_branch {
            if all_local_meta_branches.contains_key(parent_name) { // Only if parent is also av-tracked
                branches_to_keep_for_parenting.insert(parent_name.clone());
            }
        }
    }

    for (branch_name, branch_meta) in all_local_meta_branches {
        if branch_meta.pull_request.is_none() {
            debug!("Branch '{}' has no PR in metadata, skipping.", branch_name);
            continue;
        }
        let pr_meta_in_db = branch_meta.pull_request.as_ref().unwrap();

        debug!("Checking PR #{} for branch '{}'...", pr_meta_in_db.number, branch_name);
        let pr_status = match gh_client.get_pull_request_status(&repo_meta.owner.login, &repo_meta.name, pr_meta_in_db.number).await {
            Ok(status) => status,
            Err(e) => {
                warn!("Could not fetch PR status for #{} (branch '{}'): {}. Skipping.", pr_meta_in_db.number, branch_name, e);
                continue;
            }
        };

        if matches!(pr_status.state, GhPullRequestState::Merged | GhPullRequestState::Closed) {
            info!("PR #{} for branch '{}' is {}. Tidying...", pr_status.number, branch_name, format!("{:?}", pr_status.state).to_uppercase());

            if branches_to_keep_for_parenting.contains(&branch_name) {
                info!("Branch '{}' is a parent of another av-tracked branch. Skipping deletion for safety.", branch_name);
                continue;
            }

            if opts.dry_run {
                info!("[Dry Run] Would delete local branch '{}'", branch_name);
                if av_repo.branch_exists(&branch_name, Some(opts.remote_name))? {
                    info!("[Dry Run] Would delete remote branch '{}/{}'", opts.remote_name, branch_name);
                }
                info!("[Dry Run] Would remove branch '{}' from av metadata", branch_name);
            } else {
                // Checkout default/parent if current branch is being deleted
                if av_repo.current_branch_name().as_deref() == Some(branch_name.as_str()) {
                    let checkout_target = branch_meta.parent_branch.as_deref() // Prefer direct parent from meta
                        .or_else(|| av_repo.default_branch_shorthand(opts.remote_name).ok().as_deref()) // Then default for specified remote
                        .or_else(|| av_repo.default_branch_shorthand("origin").ok().as_deref()) // Fallback to origin's default
                        .unwrap_or("main"); // Ultimate fallback, though default_branch_shorthand should ideally not fail if remote exists.

                    info!("Current branch is '{}', checking out '{}' before deletion.", branch_name, checkout_target);
                    if let Err(e) = av_repo.checkout_branch(checkout_target, false, None) {
                        warn!("Failed to checkout branch '{}' before deleting '{}': {}. Git branch deletion will be skipped.", checkout_target, branch_name, e);
                        // Continue to metadata deletion.
                    } else {
                        // Only delete git branch if checkout was successful
                        delete_git_branch(av_repo, &branch_name)?;
                    }
                } else { // Not current branch, safe to delete directly
                    delete_git_branch(av_repo, &branch_name)?;
                }

                // Delete remote branch
                if av_repo.branch_exists(&branch_name, Some(opts.remote_name))? {
                    let remote_ref_to_delete = format!(":refs/heads/{}", branch_name); // Standard refspec for deleting remote branch
                    match av_repo.push(opts.remote_name, &[&remote_ref_to_delete], false) { // false for force, as it's a delete refspec
                        Ok(_) => info!("Deleted remote branch '{}/{}'.", opts.remote_name, branch_name),
                        Err(e) => warn!("Failed to delete remote branch '{}/{}': {}", opts.remote_name, branch_name, e),
                    }
                }

                // Delete from metadata
                if db.delete_branch(&branch_name)?.is_some() {
                    info!("Removed branch '{}' from av metadata.", branch_name);
                }
            }
            tidied_branches.push(branch_name.clone());
        } else {
            debug!("PR #{} for branch '{}' is {}. Skipping.", pr_status.number, branch_name, format!("{:?}", pr_status.state).to_uppercase());
        }
    }
    info!("Tidy operation complete. Processed {} branches.", tidied_branches.len()); // Log count of tidied branches
    Ok(tidied_branches)
}

// Helper to avoid code duplication for local branch deletion
fn delete_git_branch(av_repo: &AvRepo, branch_name: &str) -> Result<()> {
    match av_repo.git2_repo.find_branch(branch_name, git2::BranchType::Local) {
        Ok(mut b) => {
            b.delete().with_context(|| format!("Failed to delete local git branch '{}'", branch_name))?;
            info!("Deleted local branch '{}'.", branch_name);
        },
        Err(e) if e.code() == git2::ErrorCode::NotFound => debug!("Local git branch '{}' not found.", branch_name),
        Err(e) => warn!("Error finding local git branch '{}' for deletion: {}", branch_name, e),
    }
    Ok(())
}
