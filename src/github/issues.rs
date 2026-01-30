use super::client::GitHubClient;
use anyhow::Result;
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;

/// Information about a created/updated issue
#[derive(Debug, Clone)]
pub struct IssueInfo {
    pub id: String,      // Node ID
    pub number: u64,     // Issue number
    pub url: String,     // Web URL
}

/// Information about an existing issue
#[derive(Debug, Clone)]
pub struct ExistingIssue {
    pub id: String,
    pub number: u64,
    pub title: String,
    pub body: String,
    pub state: String,  // OPEN or CLOSED
    pub url: String,
}

/// Request to update an issue
#[derive(Debug, Clone)]
pub struct IssueUpdate {
    pub issue_id: String,
    pub title: String,
    pub body: String,
}

/// Request to create an issue
#[derive(Debug, Clone)]
pub struct IssueCreate {
    pub title: String,
    pub body: String,
    pub label_ids: Vec<String>,
}

/// Label information
#[derive(Debug, Clone)]
pub struct LabelInfo {
    pub id: String,
    pub name: String,
}

// Response types for GraphQL queries

#[derive(Deserialize)]
struct RepositoryIdResponse {
    repository: Option<RepositoryNode>,
}

#[derive(Deserialize)]
struct RepositoryNode {
    id: String,
}

#[derive(Deserialize)]
struct CreateIssueResponse {
    #[serde(rename = "createIssue")]
    create_issue: Option<CreateIssuePayload>,
}

#[derive(Deserialize)]
struct CreateIssuePayload {
    issue: Option<IssueNode>,
}

#[derive(Deserialize)]
struct IssueNode {
    id: String,
    number: u64,
    url: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    state: String,
}

#[derive(Deserialize)]
struct GetIssueResponse {
    repository: Option<GetIssueRepository>,
}

#[derive(Deserialize)]
struct GetIssueRepository {
    issue: Option<IssueNode>,
}

#[derive(Deserialize)]
struct UpdateIssueResponse {
    #[serde(rename = "updateIssue")]
    update_issue: Option<UpdateIssuePayload>,
}

#[derive(Deserialize)]
struct UpdateIssuePayload {
    issue: Option<IssueNode>,
}

#[derive(Deserialize)]
#[allow(dead_code)]
struct CloseIssueResponse {
    #[serde(rename = "closeIssue")]
    close_issue: Option<CloseIssuePayload>,
}

#[derive(Deserialize)]
#[allow(dead_code)]
struct CloseIssuePayload {
    issue: Option<IssueNode>,
}

#[derive(Deserialize)]
#[allow(dead_code)]
struct ReopenIssueResponse {
    #[serde(rename = "reopenIssue")]
    reopen_issue: Option<ReopenIssuePayload>,
}

#[derive(Deserialize)]
#[allow(dead_code)]
struct ReopenIssuePayload {
    issue: Option<IssueNode>,
}

#[derive(Deserialize)]
struct GetLabelsResponse {
    repository: Option<GetLabelsRepository>,
}

#[derive(Deserialize)]
struct GetLabelsRepository {
    labels: Option<LabelConnection>,
}

#[derive(Deserialize)]
struct LabelConnection {
    nodes: Vec<LabelNode>,
}

#[derive(Deserialize)]
struct LabelNode {
    id: String,
    name: String,
}

#[derive(Deserialize)]
struct CreateLabelResponse {
    #[serde(rename = "createLabel")]
    create_label: Option<CreateLabelPayload>,
}

#[derive(Deserialize)]
struct CreateLabelPayload {
    label: Option<LabelNode>,
}

#[derive(Deserialize)]
#[allow(dead_code)]
struct AddLabelsResponse {
    #[serde(rename = "addLabelsToLabelable")]
    add_labels: Option<AddLabelsPayload>,
}

#[derive(Deserialize)]
#[allow(dead_code)]
struct AddLabelsPayload {
    labelable: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct GetUserResponse {
    user: Option<UserNode>,
}

#[derive(Deserialize)]
struct UserNode {
    id: String,
}

impl GitHubClient {
    /// Get repository node ID
    pub async fn get_repository_id(&self, owner: &str, name: &str) -> Result<String> {
        let query = r#"
            query($owner: String!, $name: String!) {
                repository(owner: $owner, name: $name) {
                    id
                }
            }
        "#;

        let variables = json!({
            "owner": owner,
            "name": name
        });

        let response: RepositoryIdResponse = self.query(query, Some(variables)).await?;

        response
            .repository
            .map(|r| r.id)
            .ok_or_else(|| anyhow::anyhow!("Repository {}/{} not found", owner, name))
    }

    /// Get user node ID by username
    pub async fn get_user_id(&self, username: &str) -> Result<String> {
        let query = r#"
            query($login: String!) {
                user(login: $login) {
                    id
                }
            }
        "#;

        let variables = json!({ "login": username });

        let response: GetUserResponse = self.query(query, Some(variables)).await?;

        response
            .user
            .map(|u| u.id)
            .ok_or_else(|| anyhow::anyhow!("User '{}' not found", username))
    }

    /// Create a new issue
    pub async fn create_issue(
        &self,
        repo_id: &str,
        title: &str,
        body: &str,
        assignee_ids: Option<Vec<String>>,
    ) -> Result<IssueInfo> {
        let mutation = r#"
            mutation($input: CreateIssueInput!) {
                createIssue(input: $input) {
                    issue {
                        id
                        number
                        url
                    }
                }
            }
        "#;

        let mut input = json!({
            "repositoryId": repo_id,
            "title": title,
            "body": body
        });

        if let Some(ids) = assignee_ids {
            if !ids.is_empty() {
                input["assigneeIds"] = json!(ids);
            }
        }

        let variables = json!({ "input": input });

        let response: CreateIssueResponse = self.mutate(mutation, Some(variables)).await?;

        let issue = response
            .create_issue
            .and_then(|p| p.issue)
            .ok_or_else(|| anyhow::anyhow!("Failed to create issue"))?;

        Ok(IssueInfo {
            id: issue.id,
            number: issue.number,
            url: issue.url,
        })
    }

    /// Batch create multiple issues in a single request
    /// Returns results in the same order as input
    pub async fn create_issues_batch(
        &self,
        repo_id: &str,
        creates: &[IssueCreate],
        assignee_ids: Option<&[String]>,
    ) -> Result<Vec<Result<IssueInfo, String>>> {
        if creates.is_empty() {
            return Ok(Vec::new());
        }

        // Build dynamic mutation with aliases
        let mutations: Vec<String> = creates
            .iter()
            .enumerate()
            .map(|(i, _)| {
                format!(
                    "create_{i}: createIssue(input: $input_{i}) {{ issue {{ id number url }} }}"
                )
            })
            .collect();

        // Build variable definitions
        let var_defs: Vec<String> = creates
            .iter()
            .enumerate()
            .map(|(i, _)| format!("$input_{}: CreateIssueInput!", i))
            .collect();

        let mutation = format!(
            "mutation({}) {{\n  {}\n}}",
            var_defs.join(", "),
            mutations.join("\n  ")
        );

        // Build variables object
        let mut variables = serde_json::Map::new();
        for (i, create) in creates.iter().enumerate() {
            let mut input = json!({
                "repositoryId": repo_id,
                "title": create.title,
                "body": create.body
            });

            if let Some(ids) = assignee_ids {
                if !ids.is_empty() {
                    input["assigneeIds"] = json!(ids);
                }
            }

            if !create.label_ids.is_empty() {
                input["labelIds"] = json!(create.label_ids);
            }

            variables.insert(format!("input_{}", i), input);
        }

        let response: serde_json::Value = self
            .mutate(&mutation, Some(serde_json::Value::Object(variables)))
            .await?;

        let mut results = Vec::with_capacity(creates.len());
        for i in 0..creates.len() {
            let key = format!("create_{}", i);
            if let Some(data) = response.get(&key) {
                if let Some(issue) = data.get("issue") {
                    if let (Some(id), Some(number), Some(url)) = (
                        issue.get("id").and_then(|v| v.as_str()),
                        issue.get("number").and_then(|v| v.as_u64()),
                        issue.get("url").and_then(|v| v.as_str()),
                    ) {
                        results.push(Ok(IssueInfo {
                            id: id.to_string(),
                            number,
                            url: url.to_string(),
                        }));
                        continue;
                    }
                }
            }
            results.push(Err(format!("Failed to create issue {}", i)));
        }

        Ok(results)
    }

    /// Get an existing issue by number
    pub async fn get_issue(
        &self,
        owner: &str,
        name: &str,
        number: u64,
    ) -> Result<ExistingIssue> {
        let query = r#"
            query($owner: String!, $name: String!, $number: Int!) {
                repository(owner: $owner, name: $name) {
                    issue(number: $number) {
                        id
                        number
                        title
                        body
                        state
                        url
                    }
                }
            }
        "#;

        let variables = json!({
            "owner": owner,
            "name": name,
            "number": number as i64
        });

        let response: GetIssueResponse = self.query(query, Some(variables)).await?;

        let issue = response
            .repository
            .and_then(|r| r.issue)
            .ok_or_else(|| anyhow::anyhow!("Issue #{} not found in {}/{}", number, owner, name))?;

        Ok(ExistingIssue {
            id: issue.id,
            number: issue.number,
            title: issue.title,
            body: issue.body,
            state: issue.state,
            url: issue.url,
        })
    }

    /// Get multiple issues by number in a single request
    /// Returns a map of issue number -> ExistingIssue
    pub async fn get_issues_batch(
        &self,
        owner: &str,
        name: &str,
        numbers: &[u64],
    ) -> Result<std::collections::HashMap<u64, ExistingIssue>> {
        if numbers.is_empty() {
            return Ok(std::collections::HashMap::new());
        }

        // Build a dynamic query with aliases for each issue
        // e.g., issue_1: issue(number: 1) { ... }
        let issue_fields = "id number title body state url";
        let issue_queries: Vec<String> = numbers
            .iter()
            .map(|n| format!("issue_{}: issue(number: {}) {{ {} }}", n, n, issue_fields))
            .collect();

        let query = format!(
            r#"query($owner: String!, $name: String!) {{
                repository(owner: $owner, name: $name) {{
                    {}
                }}
            }}"#,
            issue_queries.join("\n                    ")
        );

        let variables = json!({
            "owner": owner,
            "name": name
        });

        let response: serde_json::Value = self.query(&query, Some(variables)).await?;

        let mut results = std::collections::HashMap::new();

        if let Some(repo) = response.get("repository") {
            for num in numbers {
                let key = format!("issue_{}", num);
                if let Some(issue_data) = repo.get(&key) {
                    if !issue_data.is_null() {
                        if let (Some(id), Some(title), Some(body), Some(state), Some(url)) = (
                            issue_data.get("id").and_then(|v| v.as_str()),
                            issue_data.get("title").and_then(|v| v.as_str()),
                            issue_data.get("body").and_then(|v| v.as_str()),
                            issue_data.get("state").and_then(|v| v.as_str()),
                            issue_data.get("url").and_then(|v| v.as_str()),
                        ) {
                            results.insert(
                                *num,
                                ExistingIssue {
                                    id: id.to_string(),
                                    number: *num,
                                    title: title.to_string(),
                                    body: body.to_string(),
                                    state: state.to_string(),
                                    url: url.to_string(),
                                },
                            );
                        }
                    }
                }
            }
        }

        Ok(results)
    }

    /// Update an existing issue
    pub async fn update_issue(
        &self,
        issue_id: &str,
        title: &str,
        body: &str,
    ) -> Result<IssueInfo> {
        let mutation = r#"
            mutation($input: UpdateIssueInput!) {
                updateIssue(input: $input) {
                    issue {
                        id
                        number
                        url
                    }
                }
            }
        "#;

        let variables = json!({
            "input": {
                "id": issue_id,
                "title": title,
                "body": body
            }
        });

        let response: UpdateIssueResponse = self.mutate(mutation, Some(variables)).await?;

        let issue = response
            .update_issue
            .and_then(|p| p.issue)
            .ok_or_else(|| anyhow::anyhow!("Failed to update issue"))?;

        Ok(IssueInfo {
            id: issue.id,
            number: issue.number,
            url: issue.url,
        })
    }

    /// Batch update multiple issues in a single request
    /// Returns a map of issue_id -> Result<IssueInfo>
    pub async fn update_issues_batch(
        &self,
        updates: &[IssueUpdate],
    ) -> Result<HashMap<String, Result<IssueInfo, String>>> {
        if updates.is_empty() {
            return Ok(HashMap::new());
        }

        // Build dynamic mutation with aliases
        let mutations: Vec<String> = updates
            .iter()
            .enumerate()
            .map(|(i, _)| {
                format!(
                    "update_{i}: updateIssue(input: $input_{i}) {{ issue {{ id number url }} }}"
                )
            })
            .collect();

        // Build variable definitions
        let var_defs: Vec<String> = updates
            .iter()
            .enumerate()
            .map(|(i, _)| format!("$input_{}: UpdateIssueInput!", i))
            .collect();

        let mutation = format!(
            "mutation({}) {{\n  {}\n}}",
            var_defs.join(", "),
            mutations.join("\n  ")
        );

        // Build variables object
        let mut variables = serde_json::Map::new();
        for (i, update) in updates.iter().enumerate() {
            variables.insert(
                format!("input_{}", i),
                json!({
                    "id": update.issue_id,
                    "title": update.title,
                    "body": update.body
                }),
            );
        }

        let response: serde_json::Value = self
            .mutate(&mutation, Some(serde_json::Value::Object(variables)))
            .await?;

        let mut results = HashMap::new();
        for (i, update) in updates.iter().enumerate() {
            let key = format!("update_{}", i);
            if let Some(data) = response.get(&key) {
                if let Some(issue) = data.get("issue") {
                    if let (Some(id), Some(number), Some(url)) = (
                        issue.get("id").and_then(|v| v.as_str()),
                        issue.get("number").and_then(|v| v.as_u64()),
                        issue.get("url").and_then(|v| v.as_str()),
                    ) {
                        results.insert(
                            update.issue_id.clone(),
                            Ok(IssueInfo {
                                id: id.to_string(),
                                number,
                                url: url.to_string(),
                            }),
                        );
                        continue;
                    }
                }
            }
            results.insert(
                update.issue_id.clone(),
                Err(format!("Failed to update issue {}", update.issue_id)),
            );
        }

        Ok(results)
    }

    /// Batch close multiple issues in a single request
    pub async fn close_issues_batch(&self, issue_ids: &[String]) -> Result<()> {
        if issue_ids.is_empty() {
            return Ok(());
        }

        let mutations: Vec<String> = issue_ids
            .iter()
            .enumerate()
            .map(|(i, _)| format!("close_{i}: closeIssue(input: $input_{i}) {{ issue {{ id }} }}"))
            .collect();

        let var_defs: Vec<String> = issue_ids
            .iter()
            .enumerate()
            .map(|(i, _)| format!("$input_{}: CloseIssueInput!", i))
            .collect();

        let mutation = format!(
            "mutation({}) {{\n  {}\n}}",
            var_defs.join(", "),
            mutations.join("\n  ")
        );

        let mut variables = serde_json::Map::new();
        for (i, issue_id) in issue_ids.iter().enumerate() {
            variables.insert(
                format!("input_{}", i),
                json!({ "issueId": issue_id }),
            );
        }

        let _: serde_json::Value = self
            .mutate(&mutation, Some(serde_json::Value::Object(variables)))
            .await?;

        Ok(())
    }

    /// Batch reopen multiple issues in a single request
    pub async fn reopen_issues_batch(&self, issue_ids: &[String]) -> Result<()> {
        if issue_ids.is_empty() {
            return Ok(());
        }

        let mutations: Vec<String> = issue_ids
            .iter()
            .enumerate()
            .map(|(i, _)| format!("reopen_{i}: reopenIssue(input: $input_{i}) {{ issue {{ id }} }}"))
            .collect();

        let var_defs: Vec<String> = issue_ids
            .iter()
            .enumerate()
            .map(|(i, _)| format!("$input_{}: ReopenIssueInput!", i))
            .collect();

        let mutation = format!(
            "mutation({}) {{\n  {}\n}}",
            var_defs.join(", "),
            mutations.join("\n  ")
        );

        let mut variables = serde_json::Map::new();
        for (i, issue_id) in issue_ids.iter().enumerate() {
            variables.insert(
                format!("input_{}", i),
                json!({ "issueId": issue_id }),
            );
        }

        let _: serde_json::Value = self
            .mutate(&mutation, Some(serde_json::Value::Object(variables)))
            .await?;

        Ok(())
    }

    /// Close an issue
    pub async fn close_issue(&self, issue_id: &str) -> Result<()> {
        let mutation = r#"
            mutation($input: CloseIssueInput!) {
                closeIssue(input: $input) {
                    issue {
                        id
                    }
                }
            }
        "#;

        let variables = json!({
            "input": {
                "issueId": issue_id
            }
        });

        let _response: CloseIssueResponse = self.mutate(mutation, Some(variables)).await?;
        Ok(())
    }

    /// Reopen an issue
    pub async fn reopen_issue(&self, issue_id: &str) -> Result<()> {
        let mutation = r#"
            mutation($input: ReopenIssueInput!) {
                reopenIssue(input: $input) {
                    issue {
                        id
                    }
                }
            }
        "#;

        let variables = json!({
            "input": {
                "issueId": issue_id
            }
        });

        let _response: ReopenIssueResponse = self.mutate(mutation, Some(variables)).await?;
        Ok(())
    }

    /// Get all labels in a repository
    pub async fn get_labels(&self, owner: &str, name: &str) -> Result<Vec<LabelInfo>> {
        let query = r#"
            query($owner: String!, $name: String!) {
                repository(owner: $owner, name: $name) {
                    labels(first: 100) {
                        nodes {
                            id
                            name
                        }
                    }
                }
            }
        "#;

        let variables = json!({
            "owner": owner,
            "name": name
        });

        let response: GetLabelsResponse = self.query(query, Some(variables)).await?;

        let labels = response
            .repository
            .and_then(|r| r.labels)
            .map(|l| {
                l.nodes
                    .into_iter()
                    .map(|n| LabelInfo {
                        id: n.id,
                        name: n.name,
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(labels)
    }

    /// Create a label in a repository
    pub async fn create_label(
        &self,
        repo_id: &str,
        name: &str,
        color: &str,
    ) -> Result<LabelInfo> {
        let mutation = r#"
            mutation($input: CreateLabelInput!) {
                createLabel(input: $input) {
                    label {
                        id
                        name
                    }
                }
            }
        "#;

        let variables = json!({
            "input": {
                "repositoryId": repo_id,
                "name": name,
                "color": color
            }
        });

        let response: CreateLabelResponse = self.mutate(mutation, Some(variables)).await?;

        let label = response
            .create_label
            .and_then(|p| p.label)
            .ok_or_else(|| anyhow::anyhow!("Failed to create label '{}'", name))?;

        Ok(LabelInfo {
            id: label.id,
            name: label.name,
        })
    }

    /// Add labels to an issue
    pub async fn add_labels_to_issue(
        &self,
        issue_id: &str,
        label_ids: &[String],
    ) -> Result<()> {
        if label_ids.is_empty() {
            return Ok(());
        }

        let mutation = r#"
            mutation($input: AddLabelsToLabelableInput!) {
                addLabelsToLabelable(input: $input) {
                    labelable {
                        __typename
                    }
                }
            }
        "#;

        let variables = json!({
            "input": {
                "labelableId": issue_id,
                "labelIds": label_ids
            }
        });

        let _response: AddLabelsResponse = self.mutate(mutation, Some(variables)).await?;
        Ok(())
    }

    /// Get or create a label, returning its ID
    pub async fn get_or_create_label(
        &self,
        owner: &str,
        name: &str,
        repo_id: &str,
        label_name: &str,
        create_if_missing: bool,
    ) -> Result<Option<String>> {
        // First try to find existing label
        let labels = self.get_labels(owner, name).await?;

        if let Some(label) = labels.iter().find(|l| l.name.eq_ignore_ascii_case(label_name)) {
            return Ok(Some(label.id.clone()));
        }

        // Label doesn't exist
        if !create_if_missing {
            return Ok(None);
        }

        // Create it with a default color
        let color = generate_label_color(label_name);
        let label = self.create_label(repo_id, label_name, &color).await?;
        Ok(Some(label.id))
    }
}

/// Generate a consistent color for a label based on its name
fn generate_label_color(name: &str) -> String {
    // Simple hash-based color generation
    let hash: u32 = name.bytes().fold(0u32, |acc, b| acc.wrapping_add(b as u32).wrapping_mul(31));
    
    // Generate a muted color (not too bright, not too dark)
    let r = ((hash >> 16) & 0xFF) % 180 + 40;
    let g = ((hash >> 8) & 0xFF) % 180 + 40;
    let b = (hash & 0xFF) % 180 + 40;
    
    format!("{:02x}{:02x}{:02x}", r, g, b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_label_color() {
        let color1 = generate_label_color("bug");
        let color2 = generate_label_color("feature");
        let color3 = generate_label_color("bug"); // Same as color1

        assert_eq!(color1.len(), 6);
        assert_eq!(color2.len(), 6);
        assert_eq!(color1, color3); // Deterministic
        assert_ne!(color1, color2); // Different inputs = different colors
    }
}
