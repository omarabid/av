use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use log::info;
use std::collections::{HashMap, VecDeque, HashSet}; // Added HashSet

use crate::git_ops::AvRepo;
use crate::meta::{BranchMeta, JsonFileDb};
use crate::{GIT_REPO, GLOBAL_CONFIG}; // For trunk branches, current branch

#[derive(Parser, Debug)]
pub struct StackOpts {
    #[clap(subcommand)]
    pub command: StackCommand,
}

#[derive(Subcommand, Debug)]
pub enum StackCommand {
    /// Display the stack tree
    Tree(StackTreeOpts),
    // TODO: Add other stack subcommands like sync, next, prev, submit
}

#[derive(Args, Debug)]
pub struct StackTreeOpts {
    // TODO: Flags like --show-revs, --all
}

pub async fn run_stack_cmd(opts: StackOpts) -> Result<()> {
    match opts.command {
        StackCommand::Tree(tree_opts) => handle_stack_tree(tree_opts).await,
    }
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
