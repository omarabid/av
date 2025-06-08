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
