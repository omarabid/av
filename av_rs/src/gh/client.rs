use reqwest::header::{AUTHORIZATION, USER_AGENT, ACCEPT};
use reqwest::Client as HttpClient;
use serde::{Deserialize, Serialize};
use anyhow::{Context, Result, anyhow};
use log::debug; // Added for logging GQL payload

use crate::gh::types::{GhRepositoryDetails, PullRequestNode}; // Added PullRequestNode

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

    pub async fn create_pull_request(
        &self,
        repository_node_id: &str,
        base_ref_name: &str,
        head_ref_name: &str,
        title: &str,
        body: &str,
        is_draft: bool,
    ) -> Result<PullRequestNode> {
        // Note: The GQL string had to be slightly modified to avoid being processed by the agent's own DSL.
        // Specifically, the # comments were removed and newlines are explicit \n.
        let mutation_str = "mutation CreatePullRequest($input: CreatePullRequestInput!) {\n  createPullRequest(input: $input) {\n    pullRequest { id number permalink }\n  }\n}";

        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct CreatePRInput<'a> {
            repository_id: &'a str,
            base_ref_name: &'a str,
            head_ref_name: &'a str,
            title: &'a str,
            body: &'a str,
            draft: bool,
        }

        #[derive(Serialize)]
        struct GqlMutationPayload<'a> {
            query: &'a str, // GQL library might call this "query" even for mutations
            variables: GqlMutationVariables<'a>,
        }

        #[derive(Serialize)]
        struct GqlMutationVariables<'a> {
            input: CreatePRInput<'a>,
        }

        // Response structs specific to CreatePullRequest
        #[derive(Deserialize, Debug)]
        struct GqlResponseCreatePR {
            data: Option<GqlResponseDataCreatePR>,
            // Re-use GqlError from get_repository_details if it's general enough
            errors: Option<Vec<GqlError>>,
        }

        #[derive(Deserialize, Debug)]
        #[serde(rename_all = "camelCase")]
        struct GqlResponseDataCreatePR {
            create_pull_request: Option<GqlResponseCreatePRDetails>,
        }

        #[derive(Deserialize, Debug)]
        #[serde(rename_all = "camelCase")]
        struct GqlResponseCreatePRDetails {
            pull_request: PullRequestNode,
        }

        let payload = GqlMutationPayload {
            query: mutation_str,
            variables: GqlMutationVariables {
                input: CreatePRInput {
                    repository_id: repository_node_id,
                    base_ref_name: base_ref_name,
                    head_ref_name: head_ref_name,
                    title: title,
                    body: body,
                    draft: is_draft,
                }
            }
        };
        debug!("Sending CreatePullRequest GQL: repo_id={}, base={}, head={}, title='{}', draft={}",
               repository_node_id, base_ref_name, head_ref_name, title, is_draft);

        let response = self.http_client
            .post(&self.graphql_url)
            // Default headers (Auth, User-Agent, Accept) should be applied by the client if configured globally,
            // or added here explicitly if not. Assuming they are set up with `HttpClient::builder()` or similar.
            // For this example, let's re-add them to be sure.
            .header(AUTHORIZATION, format!("bearer {}", self.token))
            .header(USER_AGENT, APP_USER_AGENT)
            .header(ACCEPT, "application/json")
            .json(&payload)
            .send()
            .await
            .context("Failed to send CreatePullRequest GQL request")?;

        let status = response.status();
        if !status.is_success() {
            let error_body = response.text().await.unwrap_or_else(|_| format!("Unknown error, status code {}", status));
            return Err(anyhow!(
                "CreatePullRequest GQL request failed with status {}: {}",
                status, error_body
            ));
        }

        let gql_response: GqlResponseCreatePR = response.json().await
            .context("Failed to deserialize CreatePullRequest GQL response")?;

        if let Some(errors) = gql_response.errors {
            if !errors.is_empty() {
                let error_messages: Vec<String> = errors.into_iter().map(|e| e.message).collect();
                return Err(anyhow!("GraphQL error creating PR: {}", error_messages.join(", ")));
            }
        }

        gql_response.data
            .and_then(|d| d.create_pull_request.map(|pr_details| pr_details.pull_request))
            .context("Pull request data not found in GraphQL response after creation")
    }
}
