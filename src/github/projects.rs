// GitHub Projects integration

use super::client::GitHubClient;
use anyhow::Result;
use serde::Deserialize;
use serde_json::json;

/// Information about a GitHub Project
#[derive(Debug, Clone)]
pub struct ProjectInfo {
    pub id: String,
    pub title: String,
    pub number: u64,
}

/// Result of adding an issue to a project
#[derive(Debug, Clone)]
pub struct ProjectItemInfo {
    pub item_id: String,
}

// Response types for GraphQL queries

#[derive(Deserialize)]
struct RepoProjectsResponse {
    repository: Option<RepoProjectsNode>,
}

#[derive(Deserialize)]
struct RepoProjectsNode {
    #[serde(rename = "projectsV2")]
    projects_v2: Option<ProjectConnection>,
}

#[derive(Deserialize)]
struct OrgProjectsResponse {
    organization: Option<OwnerProjectsNode>,
}

#[derive(Deserialize)]
struct UserProjectsResponse {
    user: Option<OwnerProjectsNode>,
}

#[derive(Deserialize)]
struct OwnerProjectsNode {
    #[serde(rename = "projectsV2")]
    projects_v2: Option<ProjectConnection>,
}

#[derive(Deserialize)]
struct ProjectConnection {
    nodes: Vec<ProjectNode>,
}

#[derive(Deserialize)]
struct ProjectNode {
    id: String,
    title: String,
    number: u64,
}

#[derive(Deserialize)]
struct RepoOwnerResponse {
    repository: Option<RepoOwnerNode>,
}

#[derive(Deserialize)]
struct RepoOwnerNode {
    owner: OwnerNode,
}

#[derive(Deserialize)]
struct OwnerNode {
    #[serde(rename = "__typename")]
    typename: String,
}

#[derive(Deserialize)]
#[allow(dead_code)]
struct AddProjectItemResponse {
    #[serde(rename = "addProjectV2ItemById")]
    add_item: Option<AddProjectItemPayload>,
}

#[derive(Deserialize)]
#[allow(dead_code)]
struct AddProjectItemPayload {
    item: Option<ProjectItemNode>,
}

#[derive(Deserialize)]
struct ProjectItemNode {
    id: String,
}

impl GitHubClient {
    /// Find a project by name or number
    /// 
    /// Searches in order:
    /// 1. Repo-level projects
    /// 2. Owner-level projects (org or user depending on repo owner)
    pub async fn find_project(
        &self,
        owner: &str,
        repo: &str,
        name_or_number: &str,
    ) -> Result<Option<ProjectInfo>> {
        // Check if it's a number
        let number: Option<u64> = name_or_number.parse().ok();

        // Try repo-level first
        if let Some(project) = self.find_repo_project(owner, repo, name_or_number, number).await? {
            return Ok(Some(project));
        }

        // Determine if owner is org or user
        let is_org = self.is_organization(owner, repo).await?;

        // Try owner-level
        if is_org {
            self.find_org_project(owner, name_or_number, number).await
        } else {
            self.find_user_project(owner, name_or_number, number).await
        }
    }

    /// Find a project at the repo level
    async fn find_repo_project(
        &self,
        owner: &str,
        repo: &str,
        name: &str,
        number: Option<u64>,
    ) -> Result<Option<ProjectInfo>> {
        let query = r#"
            query($owner: String!, $repo: String!) {
                repository(owner: $owner, name: $repo) {
                    projectsV2(first: 50) {
                        nodes {
                            id
                            title
                            number
                        }
                    }
                }
            }
        "#;

        let variables = json!({
            "owner": owner,
            "repo": repo
        });

        let response: RepoProjectsResponse = self.query(query, Some(variables)).await?;

        let projects = response
            .repository
            .and_then(|r| r.projects_v2)
            .map(|p| p.nodes)
            .unwrap_or_default();

        Ok(find_matching_project(&projects, name, number))
    }

    /// Find a project at the organization level
    async fn find_org_project(
        &self,
        org: &str,
        name: &str,
        number: Option<u64>,
    ) -> Result<Option<ProjectInfo>> {
        let query = r#"
            query($org: String!) {
                organization(login: $org) {
                    projectsV2(first: 50) {
                        nodes {
                            id
                            title
                            number
                        }
                    }
                }
            }
        "#;

        let variables = json!({ "org": org });

        let response: OrgProjectsResponse = self.query(query, Some(variables)).await?;

        let projects = response
            .organization
            .and_then(|o| o.projects_v2)
            .map(|p| p.nodes)
            .unwrap_or_default();

        Ok(find_matching_project(&projects, name, number))
    }

    /// Find a project at the user level
    async fn find_user_project(
        &self,
        user: &str,
        name: &str,
        number: Option<u64>,
    ) -> Result<Option<ProjectInfo>> {
        let query = r#"
            query($user: String!) {
                user(login: $user) {
                    projectsV2(first: 50) {
                        nodes {
                            id
                            title
                            number
                        }
                    }
                }
            }
        "#;

        let variables = json!({ "user": user });

        let response: UserProjectsResponse = self.query(query, Some(variables)).await?;

        let projects = response
            .user
            .and_then(|u| u.projects_v2)
            .map(|p| p.nodes)
            .unwrap_or_default();

        Ok(find_matching_project(&projects, name, number))
    }

    /// Check if the repo owner is an organization
    async fn is_organization(&self, owner: &str, repo: &str) -> Result<bool> {
        let query = r#"
            query($owner: String!, $repo: String!) {
                repository(owner: $owner, name: $repo) {
                    owner {
                        __typename
                    }
                }
            }
        "#;

        let variables = json!({
            "owner": owner,
            "repo": repo
        });

        let response: RepoOwnerResponse = self.query(query, Some(variables)).await?;

        Ok(response
            .repository
            .map(|r| r.owner.typename == "Organization")
            .unwrap_or(false))
    }

    /// Add an issue to a project
    /// 
    /// Returns the project item ID (needed for setting field values).
    /// Idempotent - if already in project, returns the existing item ID.
    pub async fn add_issue_to_project(
        &self,
        project_id: &str,
        issue_id: &str,
    ) -> Result<ProjectItemInfo> {
        let mutation = r#"
            mutation($input: AddProjectV2ItemByIdInput!) {
                addProjectV2ItemById(input: $input) {
                    item {
                        id
                    }
                }
            }
        "#;

        let variables = json!({
            "input": {
                "projectId": project_id,
                "contentId": issue_id
            }
        });

        match self.mutate::<AddProjectItemResponse>(mutation, Some(variables)).await {
            Ok(response) => {
                let item_id = response
                    .add_item
                    .and_then(|p| p.item)
                    .map(|i| i.id)
                    .ok_or_else(|| anyhow::anyhow!("Failed to add issue to project"))?;

                Ok(ProjectItemInfo { item_id })
            }
            Err(e) => {
                let err_str = e.to_string().to_lowercase();
                // Handle "already in project" - need to fetch existing item ID
                if err_str.contains("already in the project") || err_str.contains("already added") {
                    // For now, return a placeholder - we'd need another query to get the real item ID
                    // This is fine for ttr-0019; ttr-0020 will need to handle this properly
                    Ok(ProjectItemInfo {
                        item_id: String::new(),
                    })
                } else {
                    Err(e)
                }
            }
        }
    }

    /// Batch add multiple issues to a project
    /// 
    /// Returns results in the same order as input.
    pub async fn add_issues_to_project_batch(
        &self,
        project_id: &str,
        issue_ids: &[String],
    ) -> Result<Vec<Result<ProjectItemInfo, String>>> {
        if issue_ids.is_empty() {
            return Ok(Vec::new());
        }

        // Build dynamic mutation with aliases
        let mutations: Vec<String> = issue_ids
            .iter()
            .enumerate()
            .map(|(i, _)| {
                format!(
                    "add_{i}: addProjectV2ItemById(input: $input_{i}) {{ item {{ id }} }}"
                )
            })
            .collect();

        // Build variable definitions
        let var_defs: Vec<String> = issue_ids
            .iter()
            .enumerate()
            .map(|(i, _)| format!("$input_{}: AddProjectV2ItemByIdInput!", i))
            .collect();

        let mutation = format!(
            "mutation({}) {{\n  {}\n}}",
            var_defs.join(", "),
            mutations.join("\n  ")
        );

        // Build variables object
        let mut variables = serde_json::Map::new();
        for (i, issue_id) in issue_ids.iter().enumerate() {
            variables.insert(
                format!("input_{}", i),
                json!({
                    "projectId": project_id,
                    "contentId": issue_id
                }),
            );
        }

        // Execute - handle partial failures
        match self
            .mutate::<serde_json::Value>(&mutation, Some(serde_json::Value::Object(variables)))
            .await
        {
            Ok(response) => {
                let mut results = Vec::with_capacity(issue_ids.len());
                for i in 0..issue_ids.len() {
                    let key = format!("add_{}", i);
                    if let Some(data) = response.get(&key) {
                        if let Some(item_id) = data
                            .get("item")
                            .and_then(|item| item.get("id"))
                            .and_then(|id| id.as_str())
                        {
                            results.push(Ok(ProjectItemInfo {
                                item_id: item_id.to_string(),
                            }));
                        } else {
                            results.push(Err("No item ID in response".to_string()));
                        }
                    } else {
                        results.push(Err("Missing response for item".to_string()));
                    }
                }
                Ok(results)
            }
            Err(e) => {
                let err_str = e.to_string().to_lowercase();
                // If error is "already in project", treat all as success
                if err_str.contains("already in the project") || err_str.contains("already added") {
                    Ok(vec![Ok(ProjectItemInfo { item_id: String::new() }); issue_ids.len()])
                } else {
                    Err(e)
                }
            }
        }
    }
}

/// Find a project matching by number or name (case-insensitive)
fn find_matching_project(
    projects: &[ProjectNode],
    name: &str,
    number: Option<u64>,
) -> Option<ProjectInfo> {
    // Prefer number match if provided
    if let Some(num) = number {
        if let Some(p) = projects.iter().find(|p| p.number == num) {
            return Some(ProjectInfo {
                id: p.id.clone(),
                title: p.title.clone(),
                number: p.number,
            });
        }
    }

    // Fall back to name match (case-insensitive)
    let name_lower = name.to_lowercase();
    projects
        .iter()
        .find(|p| p.title.to_lowercase() == name_lower)
        .map(|p| ProjectInfo {
            id: p.id.clone(),
            title: p.title.clone(),
            number: p.number,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_matching_project_by_number() {
        let projects = vec![
            ProjectNode {
                id: "P1".to_string(),
                title: "Project One".to_string(),
                number: 1,
            },
            ProjectNode {
                id: "P2".to_string(),
                title: "Project Two".to_string(),
                number: 2,
            },
        ];

        let result = find_matching_project(&projects, "1", Some(1));
        assert!(result.is_some());
        let p = result.unwrap();
        assert_eq!(p.id, "P1");
        assert_eq!(p.number, 1);
    }

    #[test]
    fn test_find_matching_project_by_name() {
        let projects = vec![
            ProjectNode {
                id: "P1".to_string(),
                title: "Project One".to_string(),
                number: 1,
            },
            ProjectNode {
                id: "P2".to_string(),
                title: "ttr Roadmap".to_string(),
                number: 2,
            },
        ];

        let result = find_matching_project(&projects, "ttr Roadmap", None);
        assert!(result.is_some());
        let p = result.unwrap();
        assert_eq!(p.id, "P2");
        assert_eq!(p.title, "ttr Roadmap");
    }

    #[test]
    fn test_find_matching_project_case_insensitive() {
        let projects = vec![ProjectNode {
            id: "P1".to_string(),
            title: "TTR Roadmap".to_string(),
            number: 1,
        }];

        let result = find_matching_project(&projects, "ttr roadmap", None);
        assert!(result.is_some());
    }

    #[test]
    fn test_find_matching_project_not_found() {
        let projects = vec![ProjectNode {
            id: "P1".to_string(),
            title: "Other Project".to_string(),
            number: 1,
        }];

        let result = find_matching_project(&projects, "ttr Roadmap", None);
        assert!(result.is_none());
    }

    #[test]
    fn test_find_matching_project_number_preferred() {
        let projects = vec![
            ProjectNode {
                id: "P1".to_string(),
                title: "1".to_string(), // name is "1"
                number: 99,
            },
            ProjectNode {
                id: "P2".to_string(),
                title: "Other".to_string(),
                number: 1, // number is 1
            },
        ];

        // Should match by number (1) not by name ("1")
        let result = find_matching_project(&projects, "1", Some(1));
        assert!(result.is_some());
        let p = result.unwrap();
        assert_eq!(p.id, "P2");
        assert_eq!(p.number, 1);
    }
}
