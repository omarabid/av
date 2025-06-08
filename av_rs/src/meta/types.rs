use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct PullRequestMeta {
    pub id: String, // GraphQL ID
    pub number: i64,
    pub permalink: String,
    // Potentially add other fields like head_ref, base_ref if needed later
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct BranchMeta {
    pub name: String, // Should match the key in the HashMap
    pub parent_branch: Option<String>,
    pub parent_commit: Option<String>, // OID of parent branch's head when this branch was created
    pub head_commit: String, // OID of this branch's current head
    pub pull_request: Option<PullRequestMeta>,
    // Consider if `commits` relative to parent are needed explicitly,
    // or if `parent_commit` and `head_commit` are enough to derive.
    // Go's `internal/meta/branch.go` has `Parent` (name) and `Head` (OID).
    // Stack ID is implicit via parent chain for now.
}

// Re-using GhRepositoryDetails for now, but let's alias it or define a specific one if it diverges.
// For simplicity, let's assume GhRepositoryDetails is the stored form.
pub use crate::gh::GhRepositoryDetails as RepositoryMeta; // Alias for clarity

#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq)]
pub struct MetadataState {
    pub repository: Option<RepositoryMeta>,
    pub branches: HashMap<String, BranchMeta>,
}

impl MetadataState {
    pub fn branch(&self, name: &str) -> Option<&BranchMeta> {
        self.branches.get(name)
    }

    pub fn upsert_branch(&mut self, branch_meta: BranchMeta) {
        self.branches.insert(branch_meta.name.clone(), branch_meta);
    }

    pub fn delete_branch(&mut self, name: &str) -> Option<BranchMeta> {
        self.branches.remove(name)
    }

    pub fn repository(&self) -> Option<&RepositoryMeta> {
        self.repository.as_ref()
    }

    pub fn set_repository(&mut self, repo_meta: RepositoryMeta) {
        self.repository = Some(repo_meta);
    }
}
