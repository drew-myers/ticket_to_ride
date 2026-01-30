use anyhow::{Context, Result};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, USER_AGENT};
use serde::{de::DeserializeOwned, Deserialize, Serialize};

const GITHUB_GRAPHQL_URL: &str = "https://api.github.com/graphql";

/// GraphQL client for GitHub API
#[derive(Clone)]
pub struct GitHubClient {
    client: reqwest::Client,
    token: String,
}

#[derive(Serialize)]
struct GraphQLRequest<'a> {
    query: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    variables: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct GraphQLResponse<T> {
    data: Option<T>,
    errors: Option<Vec<GraphQLError>>,
}

#[derive(Deserialize, Debug)]
pub struct GraphQLError {
    pub message: String,
    #[serde(default)]
    pub path: Vec<serde_json::Value>,
    #[serde(default)]
    pub locations: Vec<ErrorLocation>,
}

#[derive(Deserialize, Debug)]
pub struct ErrorLocation {
    pub line: u32,
    pub column: u32,
}

impl std::fmt::Display for GraphQLError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)?;
        if !self.path.is_empty() {
            write!(f, " (path: {:?})", self.path)?;
        }
        Ok(())
    }
}

impl GitHubClient {
    /// Create a new GitHub client with the given token
    pub fn new(token: String) -> Result<Self> {
        let mut headers = HeaderMap::new();
        headers.insert(USER_AGENT, HeaderValue::from_static("ttr"));
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", token))
                .context("Invalid token format")?,
        );

        let client = reqwest::Client::builder()
            .default_headers(headers)
            .build()
            .context("Failed to create HTTP client")?;

        Ok(Self { client, token })
    }

    /// Execute a GraphQL query
    pub async fn query<T: DeserializeOwned>(
        &self,
        query: &str,
        variables: Option<serde_json::Value>,
    ) -> Result<T> {
        let request = GraphQLRequest { query, variables };

        let response = self
            .client
            .post(GITHUB_GRAPHQL_URL)
            .json(&request)
            .send()
            .await
            .context("Failed to send request to GitHub API")?;

        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            anyhow::bail!("GitHub API authentication failed. Check your token.");
        }

        if status == reqwest::StatusCode::FORBIDDEN {
            let text = response.text().await.unwrap_or_default();
            if text.contains("rate limit") {
                anyhow::bail!("GitHub API rate limit exceeded. Please wait and try again.");
            }
            anyhow::bail!("GitHub API forbidden: {}", text);
        }

        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            anyhow::bail!("GitHub API error ({}): {}", status, text);
        }

        let graphql_response: GraphQLResponse<T> = response
            .json()
            .await
            .context("Failed to parse GitHub API response")?;

        if let Some(errors) = graphql_response.errors {
            let error_messages: Vec<String> = errors.iter().map(|e| e.to_string()).collect();
            anyhow::bail!("GitHub GraphQL errors:\n  {}", error_messages.join("\n  "));
        }

        graphql_response
            .data
            .ok_or_else(|| anyhow::anyhow!("No data in GitHub API response"))
    }

    /// Execute a GraphQL mutation (same as query, just for semantic clarity)
    pub async fn mutate<T: DeserializeOwned>(
        &self,
        mutation: &str,
        variables: Option<serde_json::Value>,
    ) -> Result<T> {
        self.query(mutation, variables).await
    }

    /// Get the token (for debugging/testing)
    #[allow(dead_code)]
    pub fn token(&self) -> &str {
        &self.token
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_creation() {
        let client = GitHubClient::new("test_token".to_string());
        assert!(client.is_ok());
    }

    #[test]
    fn test_graphql_error_display() {
        let error = GraphQLError {
            message: "Not found".to_string(),
            path: vec![serde_json::json!("repository"), serde_json::json!("issue")],
            locations: vec![],
        };
        let display = format!("{}", error);
        assert!(display.contains("Not found"));
        assert!(display.contains("repository"));
    }
}
