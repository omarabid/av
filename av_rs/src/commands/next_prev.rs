use anyhow::{Context, Result, anyhow};
use clap::Parser;
use log::info;
use std::collections::HashMap; // For all_branches_meta

use crate::git_ops::AvRepo;
use crate::meta::{JsonFileDb, BranchMeta};
use crate::{GIT_REPO, GLOBAL_CONFIG};
use crate::utils::stack_utils;

#[derive(Parser, Debug)]
#[clap(about = "Checkout the next branch in the current stack")]
pub struct NextOpts {
    #[clap(long, help = "Checkout the trunk/root of the current stack's tree (i.e., the parent of the stack's root-most branch if that parent is a trunk)")]
    pub trunk: bool,
}

#[derive(Parser, Debug)]
#[clap(about = "Checkout the previous branch in the current stack")]
pub struct PrevOpts {
    #[clap(long, help = "Checkout the trunk/root of the current stack's tree (i.e., the parent of the stack's root-most branch if that parent is a trunk)")]
    pub trunk: bool,
}

// Helper to find the ultimate trunk parent of a given stack root.
fn get_stack_trunk_ancestor(
    stack_root_name: &str,
    all_branches_meta: &HashMap<String, BranchMeta>,
    av_repo: &AvRepo,
    config: &crate::config::AvConfig,
) -> Result<Option<String>> {
    let default_remote = config.remote.as_deref().unwrap_or("origin");
    let mut current = stack_root_name.to_string();

    // Traverse upwards from the stack_root_name using metadata,
    // until we find a branch whose parent is a trunk or has no parent in metadata.
    // That parent is our target trunk.
    loop {
        let meta = match all_branches_meta.get(&current) {
            Some(m) => m,
            None => { // `current` itself is not in metadata, it might be a trunk
                return if av_repo.is_trunk_branch(&current, default_remote)? {
                    Ok(Some(current))
                } else {
                    // Or it could be a non-av branch. If it's the initial stack_root_name, this is an issue.
                    // Otherwise, we were traversing and hit a non-av branch.
                    Ok(None)
                }
            }
        };

        if let Some(parent_name) = &meta.parent_branch {
            if av_repo.is_trunk_branch(parent_name, default_remote)? {
                return Ok(Some(parent_name.clone())); // Found the trunk parent
            }
            // If parent is in metadata but not a trunk, continue traversing.
            if all_branches_meta.contains_key(parent_name) {
                current = parent_name.clone();
            } else {
                // Parent not in metadata and not a trunk, so this stack_root_name is the highest we go in av terms.
                // Its "trunk" is effectively this non-av-tracked parent.
                return Ok(Some(parent_name.clone()));
            }
        } else {
            // No parent in metadata. This branch itself could be a trunk.
            return if av_repo.is_trunk_branch(&meta.name, default_remote)? {
                 Ok(Some(meta.name.clone()))
            } else {
                // It's a root of a feature stack, no actual trunk parent defined in av.
                // Try to get repository's default branch as a fallback.
                Ok(av_repo.default_branch_shorthand(default_remote).ok())
            }
        }
    }
}


pub async fn run_next_cmd(opts: NextOpts) -> Result<()> {
    let av_repo = GIT_REPO.get().unwrap().as_ref().context("This command requires to be run inside a Git repository")?;
    let config = GLOBAL_CONFIG.get().expect("GLOBAL_CONFIG not initialized");
    let db = JsonFileDb::new(&av_repo.common_dir);
    let all_branches_meta = db.get_all_branches().context("Failed to load branch metadata")?;
    let current_branch_name = av_repo.current_branch_name()?;

    if opts.trunk {
        let stack_members_names = stack_utils::get_current_stack_branches(&current_branch_name, &all_branches_meta, av_repo, config)?;
        if stack_members_names.is_empty() {
            info!("Current branch '{}' is not part of a known av stack. Cannot determine trunk. Trying repository default.", current_branch_name);
            let default_remote = config.remote.as_deref().unwrap_or("origin");
            let target_trunk = av_repo.default_branch_shorthand(default_remote)?;
             av_repo.checkout_branch(&target_trunk, false, None)?;
             info!("Checked out repository default trunk '{}'.", target_trunk);
            return Ok(());
        }

        // Find the root of the current stack first
        let mut stack_root_name = current_branch_name.clone();
        let mut temp_curr = current_branch_name.clone();
        let default_remote = config.remote.as_deref().unwrap_or("origin");
         while let Some(meta_curr) = all_branches_meta.get(&temp_curr) {
             if let Some(p_name) = &meta_curr.parent_branch {
                 // Only continue up if parent is part of the determined stack AND not a trunk
                 if stack_members_names.contains(p_name) && !av_repo.is_trunk_branch(p_name, default_remote)? {
                     stack_root_name = p_name.clone(); // p_name is higher in the stack
                     temp_curr = p_name.clone();
                 } else { break; } // Parent is trunk or not in this stack, so temp_curr was the root
             } else { break; } // No parent, temp_curr is the root
         }

        match get_stack_trunk_ancestor(&stack_root_name, &all_branches_meta, av_repo, config)? {
            Some(target_trunk) => {
                info!("Checking out trunk branch '{}' for current stack...", target_trunk);
                av_repo.checkout_branch(&target_trunk, false, None)?;
                info!("Checked out trunk '{}'.", target_trunk);
            }
            None => info!("Could not determine a clear trunk for the current stack of '{}'. Staying on current branch.", current_branch_name),
        }
        return Ok(());
    }

    // Normal `next`
    let mut children: Vec<&BranchMeta> = all_branches_meta.values()
        .filter(|meta| meta.parent_branch.as_deref() == Some(current_branch_name.as_str()))
        .collect();

    if children.is_empty() {
        info!("Already at the top of the stack (branch '{}' has no children in av metadata).", current_branch_name);
        return Ok(());
    }
    children.sort_by_key(|b| &b.name); // Sort for deterministic behavior
    let target_branch = &children[0].name;
    info!("Checking out next branch '{}'...", target_branch);
    av_repo.checkout_branch(target_branch, false, None)?;
    info!("Checked out next branch '{}'.", target_branch);
    Ok(())
}

pub async fn run_prev_cmd(opts: PrevOpts) -> Result<()> {
    let av_repo = GIT_REPO.get().unwrap().as_ref().context("This command requires to be run inside a Git repository")?;
    let config = GLOBAL_CONFIG.get().expect("GLOBAL_CONFIG not initialized");
    let db = JsonFileDb::new(&av_repo.common_dir);
    let all_branches_meta = db.get_all_branches().context("Failed to load branch metadata")?;
    let current_branch_name = av_repo.current_branch_name()?;

    if opts.trunk { // Same logic as next --trunk
        let stack_members_names = stack_utils::get_current_stack_branches(&current_branch_name, &all_branches_meta, av_repo, config)?;
        if stack_members_names.is_empty() {
            info!("Current branch '{}' is not part of a known av stack. Cannot determine trunk. Trying repository default.", current_branch_name);
            let default_remote = config.remote.as_deref().unwrap_or("origin");
            let target_trunk = av_repo.default_branch_shorthand(default_remote)?;
            av_repo.checkout_branch(&target_trunk, false, None)?;
            info!("Checked out repository default trunk '{}'.", target_trunk);
            return Ok(());
        }

        let mut stack_root_name = current_branch_name.clone();
        let mut temp_curr = current_branch_name.clone();
        let default_remote = config.remote.as_deref().unwrap_or("origin");

        while let Some(meta_curr) = all_branches_meta.get(&temp_curr) {
            if let Some(p_name) = &meta_curr.parent_branch {
                if stack_members_names.contains(p_name) && !av_repo.is_trunk_branch(p_name, default_remote)? {
                    stack_root_name = p_name.clone();
                    temp_curr = p_name.clone();
                } else { break; }
            } else { break; }
        }

        match get_stack_trunk_ancestor(&stack_root_name, &all_branches_meta, av_repo, config)? {
            Some(target_trunk) => {
                info!("Checking out trunk branch '{}' for current stack...", target_trunk);
                av_repo.checkout_branch(&target_trunk, false, None)?;
                info!("Checked out trunk '{}'.", target_trunk);
            }
            None => info!("Could not determine a clear trunk for the current stack of '{}'. Staying on current branch.", current_branch_name),
        }
        return Ok(());
    }

    // Normal `prev`
    let current_branch_meta = all_branches_meta.get(&current_branch_name)
        .context(format!("Current branch '{}' not found in av metadata.", current_branch_name))?;

    if let Some(parent_name) = &current_branch_meta.parent_branch {
        // Check if parent exists in Git. It might be a trunk branch not in av metadata.
        let default_remote = config.remote.as_deref().unwrap_or("origin");
        if av_repo.branch_exists(parent_name, None)? || av_repo.branch_exists(parent_name, Some(default_remote))? {
            info!("Checking out previous branch '{}'...", parent_name);
            av_repo.checkout_branch(parent_name, false, None)?;
            info!("Checked out previous branch '{}'.", parent_name);
        } else {
            return Err(anyhow!("Parent branch '{}' from metadata does not exist in Git.", parent_name));
        }
    } else {
        info!("Already at the bottom of the stack (branch '{}' has no parent in av metadata).", current_branch_name);
    }
    Ok(())
}
