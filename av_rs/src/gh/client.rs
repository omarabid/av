use reqwest::header::{AUTHORIZATION, USER_AGENT, ACCEPT};
use reqwest::Client as HttpClient;
use serde::{Deserialize, Serialize};
use anyhow::{Context, Result, anyhow};

use crate::gh::types::GhRepositoryDetails; // Corrected path

const APP_USER_AGENT: &str = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));

// Structs for GraphQL query and response
#[derive(Serialize)]
struct GqlQuery<'a> {
    query: String,
    variables: RepoVariables<'a>,
}

#[derive(Serialize)]
struct RepoVariables<'a> {
    owner: &'a str,
    name: &'a str,
}

#[derive(Deserialize, Debug)]
struct GqlResponse {
    data: Option<GqlResponseData>,
    errors: Option<Vec<GqlError>>,
}

#[derive(Deserialize, Debug)]
struct GqlError {
    message: String,
    // Can add other fields like 'type', 'path', 'locations' if needed for better error reporting
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")] // To match GraphQL typical response for fields like repository
struct GqlResponseData {
    repository: Option<GhRepositoryDetails>,
}

pub struct GhClient {
    http_client: HttpClient,
    graphql_url: String,
    token: String, // Store the token
}

impl GhClient {
    pub fn new(token: &str, base_url_override: Option<&str>) -> Result<Self> {
        let base_url = base_url_override.unwrap_or("https://api.github.com");
        let graphql_url = format!("{}/graphql", base_url);

        let http_client = HttpClient::builder()
            .build()
            .context("Failed to build Reqwest HTTP client")?;

        Ok(Self {
            http_client,
            graphql_url,
            token: token.to_string(),
        })
    }

    pub async fn get_repository_details(&self, owner: &str, name: &str) -> Result<GhRepositoryDetails> {
        let query_str = r#"
            query RepositoryDetails($owner: String!, $name: String!) {
                repository(owner: $owner, name: $name) {
                    id
                    name
                    owner {
                        login
                    }
                }
            }
        "#;

        let gql_payload = GqlQuery {
            query: query_str.to_string(),
            variables: RepoVariables { owner, name },
        };

        let response = self.http_client
            .post(&self.graphql_url)
            .header(AUTHORIZATION, format!("bearer {}", self.token))
            .header(USER_AGENT, APP_USER_AGENT)
            .header(ACCEPT, "application/json") // Ensure we ask for JSON
            .json(&gql_payload)
            .send()
            .await
            .context("Failed to send GraphQL request to GitHub")?;

        // Check for HTTP errors first
        let status = response.status();
        if !status.is_success() {
            let error_body = response.text().await.unwrap_or_else(|_| String::from("Failed to read error body"));
            return Err(anyhow!(
                "GitHub API request failed with status {}: {}",
                status,
                error_body
            ));
        }

        let gql_response: GqlResponse = response.json().await.context("Failed to deserialize GitHub GraphQL response")?;

        if let Some(errors) = gql_response.errors {
            if !errors.is_empty() {
                let error_messages: Vec<String> = errors.into_iter().map(|e| e.message).collect();
                return Err(anyhow!("GraphQL query returned errors: {}", error_messages.join(", ")));
            }
        }

        gql_response
            .data
            .and_then(|d| d.repository)
            .context("No repository data found in GraphQL response or data was null")
    }
}
