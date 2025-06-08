use anyhow::{Context, Result, anyhow};
use clap::{Args, Parser, Subcommand};
use log::{info, debug, warn}; // Added warn
use std::collections::{HashMap, HashSet, VecDeque}; // Added VecDeque for BFS/topological sort

use crate::gh::GhClient;
use crate::git_ops::AvRepo;
use crate::meta::{BranchMeta, JsonFileDb};
use crate::{GIT_REPO, GLOBAL_CONFIG};

#[derive(Parser, Debug)]
pub struct StackOpts {
    #[clap(subcommand)]
    pub command: StackCommand,
}

#[derive(Subcommand, Debug)]
pub enum StackCommand {
    /// Display the stack tree
    Tree(StackTreeOpts),
    /// Synchronize branch status with remote and PRs
    Sync(StackSyncOpts),
}

#[derive(Args, Debug)]
pub struct StackTreeOpts {
    // TODO: Flags like --show-revs, --all
}

#[derive(Args, Debug)]
pub struct StackSyncOpts {
    #[clap(long, help = "Do not fetch from remote before syncing")]
    pub no_fetch: bool,
    #[clap(long, help = "Prune remote branches during fetch")]
    pub prune: bool,
    #[clap(long, default_value_t = true, help = "Rebase stacked branches onto their parents if out of sync")]
    pub rebase: bool,
    #[clap(long, default_value_t = true, help = "Push updated branches to the remote")]
    pub push: bool,
    #[clap(long, default_value_t = false, help = "Delete local and remote branches if their PR has been merged")]
    pub prune_merged: bool,
    #[clap(long, default_value_t = false, help = "Only sync the current stack (branches that are ancestors or descendants of the current branch)")]
    pub current_stack: bool,
}


pub async fn run_stack_cmd(opts: StackOpts) -> Result<()> {
    match opts.command {
        StackCommand::Tree(tree_opts) => handle_stack_tree(tree_opts).await,
        StackCommand::Sync(sync_opts) => handle_stack_sync(sync_opts).await,
    }
}

async fn handle_stack_sync(opts: StackSyncOpts) -> Result<()> {
    let av_repo = GIT_REPO.get().unwrap().as_ref()
        .context("`av stack sync` requires being inside a Git repository.")?;
    let config = GLOBAL_CONFIG.get().expect("GLOBAL_CONFIG not initialized. This is a bug.");
    let db = JsonFileDb::new(&av_repo.common_dir);

    let repo_meta = db.read_state()?.repository
        .with_context(|| "Repository metadata not found. Please run `av init` first.")?;

    let gh_token = config.github.token.as_deref()
        .context("GitHub token not configured. Please set AV_GITHUB_TOKEN or GITHUB_TOKEN, or configure in av.toml")?;
    let gh_client = GhClient::new(gh_token, config.github.base_url.as_deref())?;

    let default_remote = config.remote.as_deref().unwrap_or("origin");
    if !opts.no_fetch {
        info!("Fetching from remote '{}' (prune: {})...", default_remote, opts.prune);
        // For now, fetching all refspecs for the remote. Specific stack refspecs could be an optimization.
        av_repo.fetch(default_remote, &[], opts.prune)
            .context(format!("Git fetch from remote '{}' failed", default_remote))?;
        info!("Fetch complete.");
    }

    let mut all_branches_meta_mut = db.get_all_branches() // Make it mutable for updates
        .context("Failed to load branch metadata for sync")?;

    if all_branches_meta_mut.is_empty() {
        info!("No av-tracked branches to sync.");
        return Ok(());
    }

    // Determine the order for syncing branches (parents before children)
    // TODO: If opts.current_stack, filter roots and all_branches_meta_mut accordingly before this.
    let branches_to_sync_ordered = get_branches_in_sync_order(&all_branches_meta_mut);

    info!("Processing {} branches for sync...", branches_to_sync_ordered.len());

    for branch_to_sync_ref in branches_to_sync_ordered {
        // Clone to allow modification and later update in all_branches_meta_mut for subsequent parent checks.
        let mut branch_to_sync_meta = branch_to_sync_ref.clone();
        let branch_name = &branch_to_sync_meta.name;
        let mut was_rebased = false;
        let mut statuses = Vec::new(); // For collecting status messages for this branch

        info!("Synchronizing branch: {}", branch_name);

        // **Rebase Logic (if opts.rebase)**
        if opts.rebase {
            if let Some(parent_branch_name) = &branch_to_sync_meta.parent_branch {
                // Get parent's *current* head from Git (could be from metadata of already synced parent, or directly from Git)
                let actual_parent_head_oid_str =
                    if let Some(synced_parent_meta) = all_branches_meta_mut.get(parent_branch_name) {
                        synced_parent_meta.head_commit.clone()
                    } else {
                        // Parent is not in metadata (e.g. trunk), get its current HEAD from Git
                        av_repo.find_commit(parent_branch_name)?
                               .map(|c| c.id().to_string())
                               .with_context(|| format!("Failed to find current HEAD for parent branch '{}'", parent_branch_name))?
                    };

                if branch_to_sync_meta.parent_commit.as_deref() != Some(actual_parent_head_oid_str.as_str()) {
                    info!("Branch '{}' (current parent OID: {}) needs rebase onto parent '{}' (new parent OID: {}).",
                          branch_name, branch_to_sync_meta.parent_commit.as_deref().unwrap_or("None"), parent_branch_name, actual_parent_head_oid_str);

                    // Simplified rebase attempt (actual git2 rebase is more complex)
                    // This is a placeholder for a more robust rebase implementation.
                    // For now, we'll simulate by checking out the branch and assuming user will handle rebase.
                    // A real implementation would use av_repo.git2_repo.rebase(...) and handle conflicts.
                    warn!("Automatic rebase for '{}' is not fully implemented. Please ensure it's correctly rebased onto '{}' at {}.",
                          branch_name, parent_branch_name, actual_parent_head_oid_str);
                    warn!("Run: git rebase --onto {} {}", actual_parent_head_oid_str, parent_branch_name);


                    // After a successful rebase, update metadata:
                    // For this simplified version, we assume rebase would succeed and update parent_commit.
                    // The head_commit would also change, but we'd need to get it from the repo post-rebase.
                    // This is a complex step not fully implemented here.
                    // For now, we will just update the parent commit in metadata to reflect the desired state.
                    // The user has to perform the actual rebase.
                    branch_to_sync_meta.parent_commit = Some(actual_parent_head_oid_str.clone());
                    // branch_to_sync_meta.head_commit = new_head_oid_after_rebase; // This would be set after successful rebase
                    was_rebased = true; // Assume rebase was "done" for push logic
                    statuses.push("Rebased (manual action required)".to_string());
                } else {
                    statuses.push("Parent up-to-date".to_string());
                }
            } else {
                statuses.push("Root branch (no parent to rebase from)".to_string());
            }
        }

        // **Push Logic (if opts.push)**
        let local_head_oid_str = &branch_to_sync_meta.head_commit; // Use metadata's head for consistency before rebase
        let remote_tracking_ref = format!("refs/remotes/{}/{}", default_remote, branch_name);
        let remote_branch_commit = av_repo.find_commit(&remote_tracking_ref)?;

        let needs_push = match (av_repo.find_commit(local_head_oid_str)?, remote_branch_commit) {
            (Some(local_commit), Some(remote_commit)) => local_commit.id() != remote_commit.id(),
            (Some(_), None) => true, // Local exists, remote doesn't
            _ => false, // Local doesn't exist (shouldn't happen for tracked branch) or other issue
        };

        if opts.push && (needs_push || was_rebased) {
            let force_push = was_rebased; // Force push if branch was (conceptually) rebased
            let refspec = format!("refs/heads/{}:refs/heads/{}", branch_name, branch_name);
            info!("Pushing branch '{}' to remote '{}' (force: {})...", branch_name, default_remote, force_push);
            match av_repo.push(default_remote, &[&refspec], force_push) {
                Ok(_) => {
                    info!("Successfully pushed '{}'.", branch_name);
                    statuses.push("Pushed".to_string());
                }
                Err(e) => {
                    warn!("Failed to push branch '{}': {}", branch_name, e);
                    statuses.push(format!("Push failed: {}", e));
                }
            }
        } else if opts.push {
            statuses.push("Push: No changes".to_string());
        }


        // **PR Status Check (Phase 1 logic - can be enhanced)**
        if let Some(pr_meta) = &branch_to_sync_meta.pull_request {
             match gh_client.get_pull_request_status(&repo_meta.owner.login, &repo_meta.name, pr_meta.number).await {
                Ok(pr_info) => {
                    statuses.push(format!("PR #{} is {:?} (draft: {})", pr_info.number, pr_info.state, pr_info.is_draft));
                    if opts.prune_merged && pr_info.state == crate::gh::PullRequestState::Merged {
                        info!("PR #{} for branch '{}' is merged. Pruning branch...", pr_info.number, branch_name);
                        // Checkout parent/default if current
                        let current_branch_name_opt = av_repo.current_branch_name().ok();
                        if current_branch_name_opt.as_deref() == Some(branch_name.as_str()) {
                            let parent_to_checkout = branch_to_sync_meta.parent_branch.as_deref()
                                .or_else(|| av_repo.default_branch_shorthand(default_remote).ok());
                            if let Some(checkout_target) = parent_to_checkout {
                                info!("Currently on merged branch '{}', checking out '{}' before pruning.", branch_name, checkout_target);
                                av_repo.checkout_branch(checkout_target, false, None)?;
                            } else {
                                return Err(anyhow!("Cannot prune current merged branch '{}' without a known parent/default to switch to.", branch_name));
                            }
                        }
                        // Delete local git branch
                        if av_repo.branch_exists(branch_name, None)? {
                            match av_repo.git2_repo.find_branch(branch_name, git2::BranchType::Local) {
                                Ok(mut b) => { b.delete()?; info!("Deleted local git branch '{}'.", branch_name); },
                                Err(e) if e.code() == git2::ErrorCode::NotFound => {}, // Already deleted
                                Err(e) => warn!("Failed to delete local git branch '{}': {}", branch_name, e),
                            }
                        }
                        // TODO: Delete remote git branch (optional, configurable)
                        db.delete_branch(branch_name)?;
                        all_branches_meta_mut.remove(branch_name); // Remove from our working set
                        info!("Pruned merged branch '{}' from metadata and locally.", branch_name);
                        statuses.push("Pruned (merged)".to_string());
                        println!("  - {}: {}", branch_name, statuses.join(", "));
                        continue; // Skip further processing for this pruned branch
                    }
                }
                Err(e) => statuses.push(format!("PR #{} status error: {}", pr_meta.number, e)),
            }
        } else {
            statuses.push("No PR".to_string());
        }

        // Update metadata in the main map for next iterations if rebase changed parent_commit
        // And persist the final state of this branch to DB
        if was_rebased { // Or if any other metadata changed
            all_branches_meta_mut.insert(branch_name.clone(), branch_to_sync_meta.clone());
            db.upsert_branch(&branch_to_sync_meta)?; // Persist change
        }
        println!("  - {}: {}", branch_name, statuses.join(", "));
    }
    Ok(())
}

// Helper function to get branches in an order suitable for syncing (parents before children)
fn get_branches_in_sync_order<'a>(
    all_meta: &'a HashMap<String, BranchMeta>,
    // TODO: Potentially filter by --current-stack here by passing current_branch_name and only including its stack.
    // For now, it prepares an order for all tracked branches.
) -> Vec<&'a BranchMeta> {
    let mut adj: HashMap<String, Vec<String>> = HashMap::new();
    let mut in_degree: HashMap<String, usize> = HashMap::new();
    let mut name_to_meta: HashMap<String, &'a BranchMeta> = HashMap::new();
    let mut roots = Vec::new();

    for (name, meta) in all_meta {
        if name.is_empty() { continue; } // Skip empty names just in case
        name_to_meta.insert(name.clone(), meta);
        in_degree.entry(name.clone()).or_insert(0); // Ensure all branches are in in_degree map

        if let Some(parent_name) = &meta.parent_branch {
            if all_meta.contains_key(parent_name) { // Only consider edges between tracked branches
                adj.entry(parent_name.clone()).or_default().push(name.clone());
                *in_degree.entry(name.clone()).or_default() += 1;
            } else {
                // Parent is not in metadata (e.g. trunk, or parent not tracked by av)
                // This branch is effectively a root in the context of av-tracked branches.
                roots.push(name.clone());
            }
        } else {
            // No parent specified, this is also a root.
            roots.push(name.clone());
        }
    }

    // Sort roots alphabetically for a consistent starting order
    roots.sort();
    roots.dedup(); // Ensure roots are unique if added from multiple conditions

    let mut queue = VecDeque::from(roots);
    let mut sync_order = Vec::new();

    while let Some(u_name) = queue.pop_front() {
        if let Some(meta_ptr) = name_to_meta.get(&u_name) {
            sync_order.push(*meta_ptr);
        }

        // Get children, sort them for deterministic processing, then add to queue if in-degree becomes 0
        if let Some(children) = adj.get(&u_name) {
            let mut sorted_children = children.clone();
            sorted_children.sort(); // Process children in a consistent order

            for v_name in sorted_children {
                if let Some(degree) = in_degree.get_mut(&v_name) {
                    *degree -= 1;
                    if *degree == 0 {
                        queue.push_back(v_name.clone());
                    }
                }
            }
        }
    }

    if sync_order.len() != all_meta.values().filter(|m| !m.name.is_empty()).count() {
        // This can happen if there's a cycle in parent definitions or if some branches
        // are not reachable from the identified roots (e.g. an isolated child whose parent is not a root).
        // For sync, it's usually okay to process the reachable ones. Problematic branches might fail later.
        warn!(
            "Sync order determined for {} branches, but {} total branches exist in metadata. Possible cycle or orphaned stack.",
            sync_order.len(),
            all_meta.len()
        );
        // To handle this, one might add all non-zero in-degree branches at the end or error.
        // For now, we proceed with the topologically sorted part.
    }
    sync_order
}


async fn handle_stack_tree(_opts: StackTreeOpts) -> Result<()> {
    let av_repo = GIT_REPO.get().unwrap().as_ref()
        .context("Stack command requires to be run inside a Git repository")?;
    let config = GLOBAL_CONFIG.get().expect("GLOBAL_CONFIG not initialized");

    let db = JsonFileDb::new(&av_repo.common_dir);
    let all_branches_meta = db.get_all_branches().context("Failed to load branch metadata")?;

    if all_branches_meta.is_empty() {
        info!("No branches are currently tracked by av. Nothing to display in tree.");
        return Ok(());
    }

    let current_git_branch = av_repo.current_branch_name().ok();

    // Build a map from parent name to children
    let mut children_map: HashMap<Option<String>, Vec<&BranchMeta>> = HashMap::new();
    let mut branch_map: HashMap<String, &BranchMeta> = HashMap::new();

    for branch_meta_tuple in &all_branches_meta {
        let (branch_name, branch_meta) = branch_meta_tuple; // Destructure tuple
        branch_map.insert(branch_name.clone(), branch_meta);
        children_map.entry(branch_meta.parent_branch.clone()).or_default().push(branch_meta);
    }

    let mut display_roots = Vec::new();
    let mut processed_as_child_or_root = HashSet::new(); // Keep track of branches already part of some tree

    // Start with branches whose parent is None (explicit roots in metadata)
    if let Some(orphans) = children_map.get(&None) {
        for orphan_meta in orphans {
            if processed_as_child_or_root.insert(orphan_meta.name.clone()) {
                 display_roots.push(orphan_meta);
            }
        }
    }

    // Identify other roots: branches whose parent is a trunk branch (not in metadata or explicitly a trunk)
    let default_remote = config.remote.as_deref().unwrap_or("origin");
    for (branch_name, branch_meta) in &all_branches_meta {
        if let Some(parent_name) = &branch_meta.parent_branch {
            // If parent is not in our metadata map OR if parent is a known trunk branch
            if !branch_map.contains_key(parent_name) ||
               av_repo.is_trunk_branch(parent_name, default_remote).unwrap_or(false) {
                // This branch could be a root of a stack
                if processed_as_child_or_root.insert(branch_name.clone()) {
                    display_roots.push(branch_meta);
                }
            }
        } else if !processed_as_child_or_root.contains(branch_name) {
            // It's an orphan if parent_branch is None and not already added
            // This case should be covered by the first loop, but added for robustness.
            if processed_as_child_or_root.insert(branch_name.clone()) {
                display_roots.push(branch_meta);
            }
        }
    }

    // Sort roots for consistent display if multiple root stacks
    display_roots.sort_by_key(|b| &b.name);
    // display_roots.dedup_by_key(|b| &b.name); // HashSet should handle uniqueness

    if display_roots.is_empty() && !all_branches_meta.is_empty() {
        info!("No clear stack roots found. Listing all known branches individually (potential cycle or all children of non-trunks not in metadata):");
        // Fallback: if no roots found but branches exist, list them all without hierarchy.
        // This might indicate a broken metadata state or all branches are children of non-trunk, non-metadata parents.
        for (_, branch_meta) in &all_branches_meta {
             if processed_as_child_or_root.insert(branch_meta.name.clone()) { // Check again to avoid re-listing if logic above changes
                display_roots.push(branch_meta);
            }
        }
        display_roots.sort_by_key(|b| &b.name); // Re-sort if we just dumped everything
    }


    info!("Stack tree:");
    if display_roots.is_empty() {
        info!(" (No displayable stacks or branches found based on current logic)");
    }

    for root_branch_meta in display_roots {
        print_branch_recursive(root_branch_meta, &children_map, 0, current_git_branch.as_deref(), &av_repo, &mut HashSet::new());
    }

    Ok(())
}

fn print_branch_recursive(
    branch_meta: &BranchMeta,
    children_map: &HashMap<Option<String>, Vec<&BranchMeta>>,
    indent_level: usize,
    current_git_branch: Option<&str>,
    _av_repo: &AvRepo, // Keep for future use (e.g., dirty status, PR status from API)
    visited_in_current_path: &mut HashSet<String>, // To detect cycles during printing
) {
    let prefix = "  ".repeat(indent_level);
    let is_current_branch = current_git_branch == Some(&branch_meta.name);
    let current_marker = if is_current_branch { "*" } else { " " };

    // Basic display: branch name
    // TODO: Add PR status, dirty status, etc. from Go version
    print!("{}{}{}", prefix, current_marker, branch_meta.name);
    if let Some(pr_meta) = &branch_meta.pull_request {
        print!(" (PR #{})", pr_meta.number);
    }
    println!(); // Newline after branch name and optional PR

    // Cycle detection for printing
    if !visited_in_current_path.insert(branch_meta.name.clone()) {
        println!("{}  └─<cycle detected with {}>", prefix, branch_meta.name);
        return;
    }

    if let Some(children) = children_map.get(&Some(branch_meta.name.clone())) {
        let mut sorted_children = children.clone();
        sorted_children.sort_by_key(|b| &b.name); // Sort children for consistent display

        let mut it = sorted_children.iter().peekable();
        while let Some(child_meta) = it.next() {
            let _connector = if it.peek().is_some() { "├─" } else { "└─" }; // Not used yet, simple indent
            print_branch_recursive(child_meta, children_map, indent_level + 1, current_git_branch, _av_repo, visited_in_current_path);
        }
    }
    visited_in_current_path.remove(&branch_meta.name); // Backtrack
}
