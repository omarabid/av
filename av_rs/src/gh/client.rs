use reqwest::header::{AUTHORIZATION, USER_AGENT, ACCEPT};
use reqwest::Client as HttpClient;
use serde::{Deserialize, Serialize};
use anyhow::{Context, Result, anyhow};
use log::debug;

use crate::gh::types::{GhRepositoryDetails, PullRequestNode, PrStatusInfo, PullRequestDetails}; // Added PullRequestDetails

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

    pub async fn get_pull_request_status(
        &self,
        repo_owner: &str,
        repo_name: &str,
        pr_number: i64,
    ) -> Result<PrStatusInfo> {
        let query_str = "query PullRequestStatus($owner: String!, $name: String!, $prNumber: Int!) {\n  repository(owner: $owner, name: $name) {\n    pullRequest(number: $prNumber) {\n      id\n      number\n      state\n      isDraft\n      headRefName\n      baseRefName\n    }\n  }\n}";

        #[derive(Serialize)]
        struct GqlVariables<'a> {
            owner: &'a str,
            name: &'a str,
            #[serde(rename = "prNumber")] // Ensure correct serialization for GQL variable name
            pr_number: i64,
        }

        #[derive(Serialize)]
        struct GqlPayload<'a> {
            query: &'a str,
            variables: GqlVariables<'a>,
        }

        // Response structs specific to GetPullRequestStatus
        #[derive(Deserialize, Debug)]
        struct GqlResponsePRStatus {
            data: Option<GqlResponseDataPRStatus>,
            errors: Option<Vec<GqlError>>, // Re-use GqlError
        }

        #[derive(Deserialize, Debug)]
        #[serde(rename_all = "camelCase")]
        struct GqlResponseDataPRStatus {
            repository: Option<GqlResponseRepoPRStatus>,
        }

        #[derive(Deserialize, Debug)]
        #[serde(rename_all = "camelCase")]
        struct GqlResponseRepoPRStatus {
            pull_request: Option<PrStatusInfo>,
        }

        let payload = GqlPayload {
            query: query_str,
            variables: GqlVariables {
                owner: repo_owner,
                name: repo_name,
                pr_number: pr_number,
            },
        };
        debug!("Sending GetPullRequestStatus GQL: owner={}, name={}, pr_number={}", repo_owner, repo_name, pr_number);

        let response = self.http_client
            .post(&self.graphql_url)
            .header(AUTHORIZATION, format!("bearer {}", self.token))
            .header(USER_AGENT, APP_USER_AGENT)
            .header(ACCEPT, "application/json")
            .json(&payload)
            .send()
            .await
            .context("Failed to send GetPullRequestStatus GQL request")?;

        let status = response.status();
        if !status.is_success() {
            let error_body = response.text().await.unwrap_or_else(|_| format!("Unknown error, status code {}", status));
            return Err(anyhow!(
                "GetPullRequestStatus GQL request failed with status {}: {}",
                status, error_body
            ));
        }

        let gql_response: GqlResponsePRStatus = response.json().await
            .context("Failed to deserialize GetPullRequestStatus GQL response")?;

        if let Some(errors) = gql_response.errors {
            if !errors.is_empty() {
                let error_messages: Vec<String> = errors.into_iter().map(|e| e.message).collect();
                return Err(anyhow!("GraphQL error fetching PR status: {}", error_messages.join(", ")));
            }
        }

        gql_response.data
            .and_then(|d| d.repository.and_then(|r| r.pull_request))
            .with_context(|| format!("PR #{} not found or data missing in GQL response for status", pr_number))
    }

    pub async fn get_pull_request_details(
        &self,
        repo_owner: &str,
        repo_name: &str,
        pr_number: i64,
    ) -> Result<PullRequestDetails> {
        let query_str = "query PullRequestDetails($owner: String!, $name: String!, $prNumber: Int!) {\n  repository(owner: $owner, name: $name) {\n    pullRequest(number: $prNumber) {\n      id\n      body\n    }\n  }\n}";

        #[derive(Serialize)]
        struct GqlVariables<'a> {
            owner: &'a str,
            name: &'a str,
            #[serde(rename = "prNumber")]
            pr_number: i64,
        }

        #[derive(Serialize)]
        struct GqlPayload<'a> {
            query: &'a str,
            variables: GqlVariables<'a>,
        }

        #[derive(Deserialize, Debug)]
        struct GqlResponsePRDetails {
            data: Option<GqlResponseDataPRDetails>,
            errors: Option<Vec<GqlError>>,
        }

        #[derive(Deserialize, Debug)]
        #[serde(rename_all = "camelCase")]
        struct GqlResponseDataPRDetails {
            repository: Option<GqlResponseRepoPRDetails>,
        }

        #[derive(Deserialize, Debug)]
        #[serde(rename_all = "camelCase")]
        struct GqlResponseRepoPRDetails {
            pull_request: Option<PullRequestDetails>,
        }

        let payload = GqlPayload {
            query: query_str,
            variables: GqlVariables { owner: repo_owner, name: repo_name, pr_number },
        };
        debug!("Sending GetPullRequestDetails GQL: owner={}, name={}, pr_number={}", repo_owner, repo_name, pr_number);

        let response = self.http_client.post(&self.graphql_url)
            .header(AUTHORIZATION, format!("bearer {}", self.token))
            .header(USER_AGENT, APP_USER_AGENT)
            .header(ACCEPT, "application/json")
            .json(&payload)
            .send().await.context("Failed to send GetPullRequestDetails GQL request")?;

        let status = response.status();
        if !status.is_success() {
            let error_body = response.text().await.unwrap_or_else(|_| format!("Unknown error, status code {}", status));
            return Err(anyhow!("GetPullRequestDetails GQL request failed with status {}: {}", status, error_body));
        }

        let gql_resp: GqlResponsePRDetails = response.json().await.context("Failed to deserialize GetPullRequestDetails GQL response")?;
        if let Some(errors) = gql_resp.errors {
            if !errors.is_empty() {
                let error_messages: Vec<String> = errors.into_iter().map(|e| e.message).collect();
                return Err(anyhow!("GraphQL error fetching PR details: {}", error_messages.join(", ")));
            }
        }
        gql_resp.data.and_then(|d| d.repository.and_then(|r| r.pull_request))
            .with_context(|| format!("PR #{} details not found in GQL response", pr_number))
    }

    pub async fn update_pull_request(
        &self,
        pr_node_id: &str,
        title: Option<String>,
        body: Option<String>,
        _base_ref_name: Option<String>, // Base ref update is complex, handle later if needed
    ) -> Result<String> { // Returns PR Node ID
        let mutation_str = "mutation UpdatePullRequest($input: UpdatePullRequestInput!) {\n  updatePullRequest(input: $input) {\n    pullRequest { id }\n  }\n}";

        #[derive(Serialize, Default)]
        #[serde(rename_all = "camelCase")]
        struct UpdatePRInputPayload {
            pull_request_id: String,
            #[serde(skip_serializing_if = "Option::is_none")]
            title: Option<String>,
            #[serde(skip_serializing_if = "Option::is_none")]
            body: Option<String>,
            #[serde(skip_serializing_if = "Option::is_none")]
            base_ref_name: Option<String>, // Ensure this is active
        }

        #[derive(Serialize)]
        struct GqlMutationPayload<'a> {
            query: &'a str,
            variables: GqlMutationVariablesUpdate<'a>, // Use a distinct name to avoid conflicts
        }

        #[derive(Serialize)]
        struct GqlMutationVariablesUpdate<'a> { // Use a distinct name
            input: UpdatePRInputPayload, // No lifetime needed if it owns its data
        }

        // Response structure for updatePullRequest
        #[derive(Deserialize, Debug)]
        struct GqlResponseUpdatePR {
            data: Option<GqlResponseDataUpdatePR>,
            errors: Option<Vec<GqlError>>,
        }

        #[derive(Deserialize, Debug)]
        #[serde(rename_all = "camelCase")]
        struct GqlResponseDataUpdatePR {
            update_pull_request: Option<GqlResponseUpdatePRDetails>,
        }

        #[derive(Deserialize, Debug)]
        #[serde(rename_all = "camelCase")]
        struct GqlResponseUpdatePRDetails {
            pull_request: UpdatedPullRequestNode,
        }
        #[derive(Deserialize, Debug)]
        struct UpdatedPullRequestNode {
            id: String,
        }

        let mut input_payload = UpdatePRInputPayload {
            pull_request_id: pr_node_id.to_string(),
            ..Default::default()
        };
        if title.is_some() { input_payload.title = title; }
        if body.is_some() { input_payload.body = body; }
        if _base_ref_name.is_some() { input_payload.base_ref_name = _base_ref_name; }


        let gql_payload = GqlMutationPayload {
            query: mutation_str,
            variables: GqlMutationVariablesUpdate { input: input_payload },
        };

        debug!("Sending UpdatePullRequest GQL for PR ID: {}", pr_node_id);
        if gql_payload.variables.input.title.is_some() { debug!("Updating title."); }
        if gql_payload.variables.input.body.is_some() { debug!("Updating body."); }

        let response = self.http_client.post(&self.graphql_url)
            .header(AUTHORIZATION, format!("bearer {}", self.token))
            .header(USER_AGENT, APP_USER_AGENT)
            .header(ACCEPT, "application/json")
            .json(&gql_payload)
            .send().await.context("Failed to send UpdatePullRequest GQL request")?;

        let status = response.status();
        if !status.is_success() {
            let error_body = response.text().await.unwrap_or_else(|_| format!("Unknown error, status code {}", status));
            return Err(anyhow!("UpdatePullRequest GQL request failed with status {}: {}", status, error_body));
        }

        let gql_resp: GqlResponseUpdatePR = response.json().await.context("Failed to deserialize UpdatePullRequest GQL response")?;

        if let Some(errors) = gql_resp.errors {
            if !errors.is_empty() {
                let error_messages: Vec<String> = errors.into_iter().map(|e| e.message).collect();
                return Err(anyhow!("GraphQL error updating PR: {}", error_messages.join(", ")));
            }
        }

        gql_resp.data
            .and_then(|d| d.update_pull_request.map(|pr_details| pr_details.pull_request.id))
            .context("pullRequest.id not found in GraphQL response after update")
    }
}
