use anyhow::{Context, Result, anyhow};
use clap::{Args, Parser, Subcommand};
use log::{info, debug, warn}; // Added warn
use std::collections::{HashMap, HashSet, VecDeque}; // Added VecDeque for BFS/topological sort

use crate::gh::GhClient;
use crate::git_ops::AvRepo;
use crate::meta::{BranchMeta, JsonFileDb, PullRequestMeta};
use crate::{GIT_REPO, GLOBAL_CONFIG, actions}; // Added actions module
use log::error;

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
    /// Create or update pull requests for the stack
    Submit(StackSubmitOpts),
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

#[derive(Args, Debug)]
pub struct StackSubmitOpts {
    #[clap(long, help="Create all PRs as drafts")]
    pub draft: bool,
    #[clap(long, help="Only submit PRs for the current branch and its ancestors in the stack")]
    pub current: bool,
    // TODO: --no-push? For now, submit always pushes.
}

fn get_current_stack_branches<'a>(
    current_branch_name: &str,
    all_meta: &'a HashMap<String, BranchMeta>,
    av_repo: &AvRepo,
    config: &crate::config::AvConfig
) -> Result<HashSet<String>> {
    let mut current_stack_members = HashSet::new();
    if !all_meta.contains_key(current_branch_name) {
        // Current git branch is not an av-tracked branch, so no "current stack" in terms of av metadata
        debug!("Current branch '{}' is not tracked by av, so no 'current stack' to determine.", current_branch_name);
        return Ok(current_stack_members);
    }

    // 1. Find the root of the current stack by traversing upwards
    let mut current_ancestor_name = current_branch_name.to_string();
    let mut stack_root_name = current_branch_name.to_string(); // Initialize with current branch
    let default_remote = config.remote.as_deref().unwrap_or("origin");

    // Traverse upwards to find the ultimate root of this stack
    // A root is a branch that has no parent in metadata, or its parent is a trunk branch
    while let Some(meta) = all_meta.get(&current_ancestor_name) {
        stack_root_name = meta.name.clone(); // Current one being checked is part of the stack path
        if let Some(parent_name) = &meta.parent_branch {
            // If parent is also tracked by av and is NOT a trunk branch, continue upwards
            if all_meta.contains_key(parent_name) && !av_repo.is_trunk_branch(parent_name, default_remote)? {
                current_ancestor_name = parent_name.clone();
            } else {
                // Parent is a trunk or not tracked (effectively a root from av's perspective for this stack)
                break;
            }
        } else {
            // No parent in metadata, this is a root
            break;
        }
    }
    debug!("Determined stack root for '{}' is '{}'", current_branch_name, stack_root_name);
    current_stack_members.insert(stack_root_name.clone());

    // 2. Find all descendants of this root (BFS/DFS)
    let mut queue = VecDeque::new();
    queue.push_back(stack_root_name); // Start BFS from the identified stack root

    while let Some(branch_name_to_scan) = queue.pop_front() {
        // Find all branches in all_meta whose parent_branch is branch_name_to_scan
        for meta_child in all_meta.values() {
            if meta_child.parent_branch.as_deref() == Some(branch_name_to_scan.as_str()) {
                if current_stack_members.insert(meta_child.name.clone()) {
                    queue.push_back(meta_child.name.clone());
                }
            }
        }
    }
    debug!("Current stack for '{}' identified with {} members: {:?}", current_branch_name, current_stack_members.len(), current_stack_members);
    Ok(current_stack_members)
}


pub async fn run_stack_cmd(opts: StackOpts) -> Result<()> {
    match opts.command {
        StackCommand::Tree(tree_opts) => handle_stack_tree(tree_opts).await,
        StackCommand::Sync(sync_opts) => handle_stack_sync(sync_opts).await,
        StackCommand::Submit(submit_opts) => handle_stack_submit(submit_opts).await,
    }
}

async fn handle_stack_submit(opts: StackSubmitOpts) -> Result<()> {
    let av_repo = GIT_REPO.get().unwrap().as_ref()
        .context("`av stack submit` requires being inside a Git repository.")?;
    let config = GLOBAL_CONFIG.get().expect("GLOBAL_CONFIG not initialized. This is a bug.");
    let db = JsonFileDb::new(&av_repo.common_dir);
    let gh_token = config.github.token.as_deref()
        .context("GitHub token not configured. Please set AV_GITHUB_TOKEN or GITHUB_TOKEN, or configure in av.toml")?;
    let gh_client = GhClient::new(gh_token, config.github.base_url.as_deref())?;
    let repo_info_for_gh = db.read_state()?.repository
        .with_context(|| "Repository metadata not initialized. Please run `av init` first.")?;

    let mut all_branches_meta_map = db.get_all_branches()
        .context("Failed to load all branch metadata for submission process")?;
    let current_git_branch_name = av_repo.current_branch_name()?;
    let default_remote = config.remote.as_deref().unwrap_or("origin");

    // 1. Determine branches to process
    let branches_to_consider_names: HashSet<String>;
    if opts.current {
        info!("Submitting PRs for current branch '{}' and its ancestors in the stack...", current_git_branch_name);
        let mut current_stack_ancestors = HashSet::new();
        let mut q = VecDeque::new();

        if all_branches_meta_map.contains_key(&current_git_branch_name) {
            q.push_back(current_git_branch_name.clone());
            current_stack_ancestors.insert(current_git_branch_name.clone());
        } else {
            info!("Current branch '{}' is not tracked by av. Cannot determine current stack for submit.", current_git_branch_name);
            return Ok(());
        }

        while let Some(branch_name) = q.pop_front() {
            if let Some(meta) = all_branches_meta_map.get(&branch_name) {
                if let Some(parent_name) = &meta.parent_branch {
                    // Only traverse up if parent is tracked and not a trunk
                    if all_branches_meta_map.contains_key(parent_name) &&
                       !av_repo.is_trunk_branch(parent_name, default_remote)? &&
                       current_stack_ancestors.insert(parent_name.clone()) { // cycle guard
                        q.push_back(parent_name.clone());
                    }
                }
            }
        }
        branches_to_consider_names = current_stack_ancestors;
        if branches_to_consider_names.is_empty() {
            // This case should ideally be caught by the initial check on current_git_branch_name
            info!("No trackable stack ancestors found for current branch '{}'.", current_git_branch_name);
            return Ok(());
        }
        debug!("Ancestors for PR submission (current stack): {:?}", branches_to_consider_names);
    } else {
        info!("Submitting PRs for all tracked branches in all stacks...");
        branches_to_consider_names = all_branches_meta_map.keys().cloned().collect();
    }

    let ordered_branch_meta_refs = get_branches_in_sync_order(&all_branches_meta_map);
    let branches_to_submit_ordered: Vec<&BranchMeta> = ordered_branch_meta_refs.into_iter()
        .filter(|meta| branches_to_consider_names.contains(&meta.name))
        .collect();

    if branches_to_submit_ordered.is_empty() {
        info!("No branches found to submit based on criteria.");
        return Ok(());
    }

    info!("Found {} branches to process for PR submission.", branches_to_submit_ordered.len());
    let mut pr_creation_errors = Vec::new();

    for branch_meta_ref in branches_to_submit_ordered {
        let mut branch_meta = branch_meta_ref.clone(); // Clone to modify
        info!("Processing branch '{}' for PR submission...", branch_meta.name);

        if branch_meta.pull_request.is_some() {
            info!("Branch '{}' already has PR #{}. Skipping.", branch_meta.name, branch_meta.pull_request.as_ref().unwrap().number);
            continue;
        }

        let parent_branch_name_for_pr = match &branch_meta.parent_branch {
            Some(p_name) => p_name.clone(),
            None => {
                // If a root branch in metadata has no parent, its base must be a trunk.
                // Determine the actual default trunk from the repository.
                av_repo.default_branch_shorthand(default_remote)
                    .with_context(|| format!("Branch '{}' has no parent in metadata and failed to get default trunk branch to use as base for PR.", branch_meta.name))?
            }
        };

        let title = {
            let head_oid = av_repo.find_commit(&branch_meta.head_commit)?.context("Cannot find branch head commit OID from metadata")?.id();
            let parent_for_log_oid = av_repo.find_commit(&parent_branch_name_for_pr)?.context(format!("Cannot find parent branch '{}' head commit OID for log", parent_branch_name_for_pr))?.id();

            if head_oid == parent_for_log_oid {
                 warn!("Branch '{}' has no new commits compared to its parent '{}'. Using branch name as title.", branch_meta.name, parent_branch_name_for_pr);
                 branch_meta.name.clone()
            } else {
                let commits = av_repo.list_commits(head_oid, Some(parent_for_log_oid))?;
                if let Some(top_commit_oid) = commits.first() {
                    av_repo.get_commit_summary(*top_commit_oid)?
                } else {
                    debug!("No unique commits found for PR title for branch '{}' (head: {}, parent: {}). Using branch name as title.", branch_meta.name, head_oid, parent_for_log_oid);
                    branch_meta.name.clone()
                }
            }
        };
        let body = format!("PR for branch {}.", branch_meta.name); // Simple body for now

        // Construct CreatePullRequestOpts for the action
        let action_opts = actions::pr::CreatePullRequestOpts {
            repo_owner: &repo_info_for_gh.owner.login,
            repo_name: &repo_info_for_gh.name,
            repo_node_id: &repo_info_for_gh.id,
            head_ref_name: &branch_meta.name,
            base_ref_name: &parent_branch_name_for_pr,
            title: title.clone(), // title is String, action_opts takes String
            body: body.clone(),   // body is String, action_opts takes String
            is_draft: opts.draft,
            no_push: false, // stack submit implies push for now; add flag to StackSubmitOpts if needed
            default_remote_name: default_remote,
        };

        match actions::pr::create_pull_request(av_repo, &gh_client, &db, action_opts).await {
            Ok(created_pr_meta) => {
                // The action already updates the DB, but we need to update our in-memory map
                // if other iterations depend on this PR info (e.g., for complex body generation - not currently).
                if let Some(meta_in_map) = all_branches_meta_map.get_mut(&branch_meta.name) {
                    meta_in_map.pull_request = Some(created_pr_meta);
                }
                // Log success (already done by the action, but can add a specific one for submit context)
                // info!("PR for branch '{}' created/updated successfully via action.", branch_meta.name);
            }
            Err(e) => {
                let err_msg = format!("Failed to create PR for branch '{}' via action: {}", branch_meta.name, e);
                error!("{}", err_msg);
                pr_creation_errors.push(err_msg);
            }
        }
    }

    if !pr_creation_errors.is_empty() {
        error!("Encountered errors during PR submission process:");
        for err_msg in pr_creation_errors {
            error!("  - {}", err_msg);
        }
        return Err(anyhow!("One or more errors occurred during PR submission. Please check logs."));
    }

    info!("Stack submit process completed.");
    Ok(())
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

    let initial_branch_name = av_repo.current_branch_name().ok();

    let mut all_branches_meta_map = db.get_all_branches()
        .context("Failed to load branch metadata for sync")?;

    if all_branches_meta_map.is_empty() {
        info!("No av-tracked branches to sync.");
        return Ok(());
    }

    let current_git_branch_name = av_repo.current_branch_name()
        .context("Failed to get current git branch name. Ensure you are on a branch.")?;

    let branches_to_process_names: HashSet<String>;
    if opts.current_stack {
        info!("Syncing current stack (based on current branch '{}')...", current_git_branch_name);
        branches_to_process_names = get_current_stack_branches(&current_git_branch_name, &all_branches_meta_map, av_repo, config)?;
        if branches_to_process_names.is_empty() {
            info!("Current branch '{}' is not part of a known av-tracked stack or no tracked branches in its stack.", current_git_branch_name);
            return Ok(());
        }
        debug!("Will process {} branches in the current stack.", branches_to_process_names.len());
    } else {
        info!("Syncing all av-tracked branches...");
        branches_to_process_names = all_branches_meta_map.keys().cloned().collect();
    }

    let ordered_branch_meta_refs = get_branches_in_sync_order(&all_branches_meta_map);
    let mut branches_to_process_ordered: Vec<&BranchMeta> = ordered_branch_meta_refs.into_iter()
        .filter(|meta| branches_to_process_names.contains(&meta.name))
        .collect();

    if branches_to_process_ordered.is_empty() && !branches_to_process_names.is_empty() {
        // This might happen if get_branches_in_sync_order has issues with the filtered set.
        // Or if current_stack logic results in a set not properly ordered.
        // As a fallback, use the names directly if ordering failed to produce results for the filtered set.
        // This part needs careful review based on how get_branches_in_sync_order handles subsets.
        // For now, let's assume get_branches_in_sync_order gets all, and we filter AFTER.
        warn!("Sync order did not include some branches selected for processing. This might indicate an issue.");
    }


    info!("Will attempt to sync {} branches in determined order.", branches_to_process_ordered.len());

    for branch_to_sync_ref in branches_to_process_ordered {
        let mut branch_to_sync_meta = branch_to_sync_ref.clone();
        let branch_name = &branch_to_sync_meta.name;
        let mut was_rebased = false;
        let mut final_statuses = Vec::new(); // Use this for the final summary line for the branch

        // info!("Synchronizing branch: {}", branch_name); // Moved to be part of the final summary

        if opts.rebase {
            // Removed the duplicated/nested `if opts.rebase {` here
            if let Some(parent_branch_name) = &branch_to_sync_meta.parent_branch {
                let parent_display_name = parent_branch_name.clone(); // For logging
                let actual_parent_head_oid_str = all_branches_meta_map
                    .get(parent_branch_name)
                    .map(|meta| meta.head_commit.clone())
                    .or_else(|| av_repo.find_commit(parent_branch_name).ok().flatten().map(|c| c.id().to_string()))
                    .with_context(|| format!("Failed to find current HEAD for parent branch '{}'", parent_branch_name))?;

                let old_parent_commit_for_rebase = branch_to_sync_meta.parent_commit.clone();
                if old_parent_commit_for_rebase.as_deref() != Some(actual_parent_head_oid_str.as_str()) {
                    info!("Branch '{}': Checking rebase against parent '{}' (current head: {})...",
                          branch_name, parent_display_name, actual_parent_head_oid_str);

                    let old_base_commit_oid = old_parent_commit_for_rebase
                        .with_context(|| format!("Branch '{}' is missing 'parent_commit' in metadata, needed for rebase.", branch_name))?;

                    if av_repo.current_branch_name().as_deref() != Some(branch_name.as_str()) {
                        debug!("Branch '{}': Checking out to prepare for rebase.", branch_name);
                        av_repo.checkout_branch(branch_name, false, None)?;
                    }

                    match av_repo.rebase_onto(&actual_parent_head_oid_str, &old_base_commit_oid, branch_name) {
                        Ok(_) => {
                            let new_head_commit = av_repo.find_commit("HEAD")?.context("Failed to get new HEAD commit after rebase")?;
                            let new_head_short_oid = new_head_commit.id().to_string().chars().take(7).collect::<String>();
                            info!("Branch '{}': Successfully rebased onto '{}'. New head: {}.",
                                  branch_name, parent_display_name, new_head_short_oid);
                            branch_to_sync_meta.head_commit = new_head_commit.id().to_string();
                            branch_to_sync_meta.parent_commit = Some(actual_parent_head_oid_str.clone());
                            was_rebased = true;
                            final_statuses.push("Rebased".to_string());
                        }
                        Err(e) => {
                            let err_msg = format!("{}", e);
                            if err_msg.contains("Merge conflict") || err_msg.contains("Resolve all conflicts") || err_msg.contains("applied an RERERE") {
                                warn!("Branch '{}': Rebase failed due to merge conflicts. Please resolve them and run `git rebase --continue` or `git rebase --abort`. Skipping further actions for this branch.", branch_name);
                            } else {
                                warn!("Branch '{}': Automated rebase failed: {}. Skipping further actions for this branch.", branch_name, err_msg);
                            }
                            final_statuses.push(format!("Rebase Failed ({})", err_msg.split('\n').next().unwrap_or("unknown error")));
                             if let Some(orig_b) = &initial_branch_name {
                                if av_repo.current_branch_name().as_deref().is_ok() && av_repo.current_branch_name().as_deref() != Some(orig_b.as_str()) {
                                   if av_repo.checkout_branch(orig_b, false, None).is_err() { warn!("Failed to restore original branch '{}'.", orig_b); }
                                }
                            }
                            println!("  - {}: {}", branch_name, final_statuses.join(", "));
                            continue;
                        }
                    }
                } else {
                    final_statuses.push("Parent up-to-date".to_string());
                }
            } else {
                final_statuses.push("Root branch (no parent to rebase from)".to_string());
            }
        } // This now correctly closes the outer `if opts.rebase`

        let local_head_for_push_check = &branch_to_sync_meta.head_commit;
        let remote_tracking_ref_for_push = format!("refs/remotes/{}/{}", default_remote, branch_name);
        let remote_branch_commit_for_push = av_repo.find_commit(&remote_tracking_ref_for_push)?;

        let needs_push = match (av_repo.find_commit(local_head_for_push_check)?, remote_branch_commit_for_push) {
            (Some(local_commit), Some(remote_commit)) => local_commit.id() != remote_commit.id(),
            (Some(_), None) => true,
            _ => false,
        };

        if opts.push && (needs_push || was_rebased) {
            let force_push = was_rebased;
            let refspec = format!("refs/heads/{}:refs/heads/{}", branch_name, branch_name);
            info!("Branch '{}': Checking push status to remote '{}' (force: {})...", branch_name, default_remote, force_push);
            match av_repo.push(default_remote, &[&refspec], force_push) {
                Ok(_) => { info!("Branch '{}': Successfully pushed (force: {}).", branch_name, force_push); final_statuses.push("Pushed".to_string()); }
                Err(e) => { warn!("Branch '{}': Failed to push: {}", branch_name, e); final_statuses.push(format!("Push Failed ({})", e)); }
            }
        } else if opts.push {
            final_statuses.push("Push: No changes".to_string());
        }

        if let Some(pr_meta) = &branch_to_sync_meta.pull_request {
             match gh_client.get_pull_request_status(&repo_meta.owner.login, &repo_meta.name, pr_meta.number).await {
                Ok(pr_info) => {
                    final_statuses.push(format!("PR #{} {:?} (draft: {})", pr_info.number, pr_info.state, pr_info.is_draft));
                    if opts.prune_merged && pr_info.state == crate::gh::PullRequestState::Merged {
                        info!("Branch '{}': PR #{} is MERGED. Pruning...", branch_name, pr_info.number);
                        if initial_branch_name.as_deref() == Some(branch_name.as_str()) {
                            let parent_to_checkout = branch_to_sync_meta.parent_branch.as_deref().or_else(|| av_repo.default_branch_shorthand(default_remote).ok());
                            if let Some(checkout_target) = parent_to_checkout { av_repo.checkout_branch(checkout_target, false, None)?; }
                            else { return Err(anyhow!("Cannot prune current merged branch '{}'.", branch_name)); }
                        }
                        if av_repo.branch_exists(branch_name, None)? {
                            if let Ok(mut b) = av_repo.git2_repo.find_branch(branch_name, git2::BranchType::Local) { b.delete()?; }
                        }
                        db.delete_branch(branch_name)?;
                        all_branches_meta_map.remove(branch_name);
                        final_statuses.push("Pruned (merged)".to_string());
                        println!("  - {}: {}", branch_name, final_statuses.join(", "));
                        continue;
                    }
                }
                Err(e) => final_statuses.push(format!("PR #{} status error: {}", pr_meta.number, e)),
            }
        } else {
            final_statuses.push("No PR".to_string());
        }

        if was_rebased {
            all_branches_meta_map.insert(branch_name.clone(), branch_to_sync_meta.clone());
            db.upsert_branch(&branch_to_sync_meta)?;
        }
        println!("  - {}: {}", branch_name, final_statuses.join(", "));
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
