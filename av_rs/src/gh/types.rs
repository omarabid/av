use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone)] // Added Clone for convenience
pub struct GhOwner {
    pub login: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)] // Added Clone for convenience
pub struct GhRepositoryDetails {
    pub id: String,
    pub owner: GhOwner,
    pub name: String,
}

#[derive(Deserialize, Debug, Clone)] // Only Deserialize needed if it's only for GQL responses
pub struct PullRequestNode {
    pub id: String,
    pub number: i64,
    pub permalink: String,
}

#[derive(Deserialize, Debug, Clone, PartialEq, Eq)] // Added PartialEq, Eq for potential comparisons
#[serde(rename_all = "SCREAMING_SNAKE_CASE")] // Assuming GQL returns MERGED, OPEN, CLOSED
pub enum PullRequestState {
    Merged,
    Open,
    Closed,
    Unknown, // Fallback for any other state not explicitly handled
}

impl Default for PullRequestState {
    fn default() -> Self {
        PullRequestState::Unknown
    }
}


#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PrStatusInfo {
    pub id: String,
    pub number: i64,
    pub state: PullRequestState,
    pub is_draft: bool,
    pub head_ref_name: String,
    pub base_ref_name: String,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestDetails {
    pub id: String,
    pub body: Option<String>, // Body can be null
    // pub title: String, // Not fetching title in this specific struct for now, can be added
    // pub base_ref_name: Option<String>, // Also can be added if needed
}


// Types for PR Status Details
#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct GhAuthor {
    pub login: String,
}

// This was defined in the prompt but not used in the GhPullRequestDetailsForStatus struct directly.
// Individual contexts might be useful for a more detailed CI status breakdown later.
// #[derive(Deserialize, Debug, Clone)]
// #[serde(rename_all = "camelCase")]
// pub struct GhCommitStatusContext {
//     pub context: String,
//     pub state: String, // Note: This 'state' is for a specific check, not the overall rollup
//     pub target_url: Option<String>,
// }

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")] // GitHub's StatusState enum (SUCCESS, PENDING, FAILURE, ERROR)
pub struct GhStatusCheckRollupNode {
    pub state: String,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct GhCommitNode {
    pub oid: String,
    pub status_check_rollup: Option<GhStatusCheckRollupNode>,
}

// GQL `commits(last:1){edges{node{commit}}}` returns a list of edges, even for `last:1`.
// The prompt used GhCommitEdge { node: GhCommitNode }, but GQL for `commits.nodes.commit` is more direct.
// Let's adjust to match the more direct `commits { nodes { commit } }` structure for simplicity.
// GQL: headRevision { oid } or commits(last:1) { nodes { oid statusCheckRollup { state } } }
// The prompt's GQL for `GhPullRequestDetailsForStatus` used `commits(last:1) { edges { node { oid ... } } }`
// So, GhCommitEdge and GhCommitHistory are correct based on that GQL.

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct GhCommitEdge {
    pub node: GhCommitNode, // This node is the commit itself.
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct GhCommitHistory {
    // If using `nodes` directly from commits: `pub nodes: Vec<GhCommitNode>`
    // If using `edges { node { ... } }`:
    pub edges: Vec<GhCommitEdge>,
}


#[derive(Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum GhReviewDecision {
    Approved,
    ChangesRequested,
    ReviewRequired,
    Unknown, // Fallback
}

impl Default for GhReviewDecision {
    fn default() -> Self {
        GhReviewDecision::Unknown
    }
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct GhPullRequestDetailsForStatus {
    pub id: String,
    pub number: i64,
    pub title: String,
    pub author: Option<GhAuthor>, // Author can be null if user deleted account
    pub state: PullRequestState, // Reuses the existing PullRequestState enum
    pub is_draft: bool,
    pub head_ref_name: String,
    pub base_ref_name: String,
    #[serde(default)] // reviewDecision can be null (e.g. no reviews yet)
    pub review_decision: Option<GhReviewDecision>,
    pub commits: GhCommitHistory, // For `commits(last: 1) { edges { node { ... } } }`
}
