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
