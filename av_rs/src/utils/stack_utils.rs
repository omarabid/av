use crate::config::AvConfig;
use crate::git_ops::AvRepo;
use crate::meta::BranchMeta;
use anyhow::Result;
use std::collections::{HashMap, HashSet, VecDeque};
use log::debug;

pub fn get_current_stack_branches(
    current_branch_name: &str,
    all_meta: &HashMap<String, BranchMeta>,
    av_repo: &AvRepo,
    config: &AvConfig,
) -> Result<HashSet<String>> {
    let mut stack_members = HashSet::new();
    if !all_meta.contains_key(current_branch_name) {
        debug!("Branch '{}' not in metadata, cannot determine its stack.", current_branch_name);
        return Ok(stack_members);
    }

    let mut stack_root_name = current_branch_name.to_string();
    let mut ancestor_iter = current_branch_name.to_string();
    let default_remote = config.remote.as_deref().unwrap_or("origin");

    // 1. Traverse upwards to find the root and collect all ancestors
    debug!("Finding stack root for '{}': traversing upwards.", current_branch_name);
    loop {
        stack_members.insert(ancestor_iter.clone());
        stack_root_name = ancestor_iter.clone();

        let current_meta = all_meta.get(&ancestor_iter)
            .expect("Should find meta for a branch name known to be in all_meta"); // Was checked at the start

        if let Some(parent_name) = &current_meta.parent_branch {
            if all_meta.contains_key(parent_name) &&
               !av_repo.is_trunk_branch(parent_name, default_remote)? {
                ancestor_iter = parent_name.clone();
            } else {
                debug!("Root for '{}' found at '{}' (parent '{}' is trunk or not tracked).", current_branch_name, stack_root_name, parent_name);
                break;
            }
        } else {
            debug!("Root for '{}' found at '{}' (no parent in metadata).", current_branch_name, stack_root_name);
            break;
        }
    }

    // 2. Traverse downwards from the stack_root_name to find all descendants
    debug!("Finding all descendants for stack root '{}'", stack_root_name);
    let mut queue = VecDeque::new();
    // stack_root_name is already in stack_members from upward traversal.
    // Add it to the queue to start the BFS for descendants.
    if all_meta.contains_key(&stack_root_name) { // Should always be true if current_branch_name was in all_meta
        queue.push_back(stack_root_name);
    }

    // No need to re-insert the root into stack_members, it was done during upward traversal.

    while let Some(branch_name_to_scan) = queue.pop_front() {
        // Find children of branch_to_scan by iterating all_meta
        for (child_name, child_meta) in all_meta {
            if child_meta.parent_branch.as_deref() == Some(branch_name_to_scan.as_str()) {
                if stack_members.insert(child_name.clone()) { // Add if not already present
                    queue.push_back(child_name.clone());
                }
            }
        }
    }
    debug!("Determined stack for '{}': {:?}", current_branch_name, stack_members);
    Ok(stack_members)
}
