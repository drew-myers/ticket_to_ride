// Sub-issue relationship management

use super::client::GitHubClient;
use anyhow::Result;
use serde::Deserialize;
use serde_json::json;

/// A sub-issue link to create
#[derive(Debug, Clone)]
pub struct SubIssueLink {
    pub parent_issue_id: String,
    pub child_issue_id: String,
}

#[derive(Deserialize)]
#[allow(dead_code)]
struct AddSubIssueResponse {
    #[serde(rename = "addSubIssue")]
    add_sub_issue: Option<AddSubIssuePayload>,
}

#[derive(Deserialize)]
#[allow(dead_code)]
struct AddSubIssuePayload {
    #[serde(rename = "subIssue")]
    sub_issue: Option<SubIssueNode>,
}

#[derive(Deserialize)]
#[allow(dead_code)]
struct SubIssueNode {
    id: String,
}

impl GitHubClient {
    /// Add a sub-issue relationship (child under parent)
    /// 
    /// This is idempotent - if already linked, it succeeds silently.
    pub async fn add_sub_issue(
        &self,
        parent_issue_id: &str,
        child_issue_id: &str,
    ) -> Result<()> {
        let mutation = r#"
            mutation($input: AddSubIssueInput!) {
                addSubIssue(input: $input) {
                    subIssue {
                        id
                    }
                }
            }
        "#;

        let variables = json!({
            "input": {
                "issueId": parent_issue_id,
                "subIssueId": child_issue_id
            }
        });

        // Try the mutation - if already linked, GitHub returns an error
        // which we treat as success (idempotent)
        match self.mutate::<AddSubIssueResponse>(mutation, Some(variables)).await {
            Ok(_) => Ok(()),
            Err(e) => {
                let err_str = e.to_string().to_lowercase();
                // GitHub returns these errors if already linked
                if err_str.contains("already a sub-issue") || 
                   err_str.contains("is already a child") ||
                   err_str.contains("already has this sub-issue") ||
                   err_str.contains("duplicate sub-issues") ||
                   err_str.contains("may only have one parent") {
                    Ok(()) // Treat as success - idempotent
                } else {
                    Err(e)
                }
            }
        }
    }

    /// Batch add multiple sub-issue relationships in a single request
    /// 
    /// Returns a list of results in the same order as input.
    /// Each result is Ok(()) on success or Err(message) on failure.
    pub async fn add_sub_issues_batch(
        &self,
        links: &[SubIssueLink],
    ) -> Result<Vec<Result<(), String>>> {
        if links.is_empty() {
            return Ok(Vec::new());
        }

        // Build dynamic mutation with aliases
        let mutations: Vec<String> = links
            .iter()
            .enumerate()
            .map(|(i, _)| {
                format!(
                    "link_{i}: addSubIssue(input: $input_{i}) {{ subIssue {{ id }} }}"
                )
            })
            .collect();

        // Build variable definitions
        let var_defs: Vec<String> = links
            .iter()
            .enumerate()
            .map(|(i, _)| format!("$input_{}: AddSubIssueInput!", i))
            .collect();

        let mutation = format!(
            "mutation({}) {{\n  {}\n}}",
            var_defs.join(", "),
            mutations.join("\n  ")
        );

        // Build variables object
        let mut variables = serde_json::Map::new();
        for (i, link) in links.iter().enumerate() {
            variables.insert(
                format!("input_{}", i),
                json!({
                    "issueId": link.parent_issue_id,
                    "subIssueId": link.child_issue_id
                }),
            );
        }

        // Execute - handle "already linked" errors as success (idempotent)
        match self
            .mutate::<serde_json::Value>(&mutation, Some(serde_json::Value::Object(variables)))
            .await
        {
            Ok(response) => {
                let mut results = Vec::with_capacity(links.len());
                for i in 0..links.len() {
                    let key = format!("link_{}", i);
                    if let Some(data) = response.get(&key) {
                        if data.get("subIssue").is_some() {
                            results.push(Ok(()));
                        } else {
                            results.push(Err("No subIssue in response".to_string()));
                        }
                    } else {
                        // Missing from response - treat as success
                        results.push(Ok(()));
                    }
                }
                Ok(results)
            }
            Err(e) => {
                let err_str = e.to_string().to_lowercase();
                // If error is "already linked", treat all as success (idempotent)
                if err_str.contains("already a sub-issue")
                    || err_str.contains("is already a child")
                    || err_str.contains("already has this sub-issue")
                    || err_str.contains("duplicate sub-issues")
                    || err_str.contains("may only have one parent")
                {
                    Ok(vec![Ok(()); links.len()])
                } else {
                    Err(e)
                }
            }
        }
    }
}
