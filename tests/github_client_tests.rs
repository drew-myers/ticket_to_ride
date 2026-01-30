//! Integration tests for GitHubClient using wiremock
//!
//! These tests verify the HTTP-level behavior of the GitHub client,
//! including request formatting, response parsing, and error handling.

use serde_json::json;
use ticket_to_ride::github::client::GitHubClient;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Helper to create a client pointing to the mock server
fn create_test_client(server: &MockServer) -> GitHubClient {
    GitHubClient::with_base_url("test_token".to_string(), server.uri()).unwrap()
}

/// Helper for GraphQL response with data
fn graphql_response<T: serde::Serialize>(data: T) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({ "data": data }))
}

/// Helper for GraphQL response with errors
fn graphql_error_response(message: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "data": null,
        "errors": [{ "message": message, "path": [], "locations": [] }]
    }))
}

// =============================================================================
// Basic Query/Mutation Tests
// =============================================================================

#[tokio::test]
async fn test_query_success() {
    let server = MockServer::start().await;
    let client = create_test_client(&server);

    Mock::given(method("POST"))
        .and(path("/"))
        .and(header("authorization", "Bearer test_token"))
        .and(header("user-agent", "ttr"))
        .respond_with(graphql_response(json!({
            "repository": {
                "id": "R_123",
                "name": "test-repo"
            }
        })))
        .expect(1)
        .mount(&server)
        .await;

    #[derive(serde::Deserialize)]
    struct Response {
        repository: Repository,
    }
    #[derive(serde::Deserialize)]
    struct Repository {
        id: String,
        name: String,
    }

    let result: Response = client
        .query(
            "query { repository(owner: \"test\", name: \"test-repo\") { id name } }",
            None,
        )
        .await
        .unwrap();

    assert_eq!(result.repository.id, "R_123");
    assert_eq!(result.repository.name, "test-repo");
}

#[tokio::test]
async fn test_query_with_variables() {
    let server = MockServer::start().await;
    let client = create_test_client(&server);

    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(graphql_response(json!({
            "user": { "id": "U_456" }
        })))
        .expect(1)
        .mount(&server)
        .await;

    #[derive(serde::Deserialize)]
    struct Response {
        user: User,
    }
    #[derive(serde::Deserialize)]
    struct User {
        id: String,
    }

    let variables = json!({ "login": "testuser" });
    let result: Response = client
        .query(
            "query($login: String!) { user(login: $login) { id } }",
            Some(variables),
        )
        .await
        .unwrap();

    assert_eq!(result.user.id, "U_456");
}

// =============================================================================
// Error Handling Tests
// =============================================================================

#[tokio::test]
async fn test_unauthorized_error() {
    let server = MockServer::start().await;
    let client = create_test_client(&server);

    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(401).set_body_string("Bad credentials"))
        .mount(&server)
        .await;

    #[derive(serde::Deserialize, Debug)]
    struct Empty {}

    let result: Result<Empty, _> = client.query("query { viewer { id } }", None).await;

    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("authentication failed"), "Error was: {}", err);
}

#[tokio::test]
async fn test_rate_limit_error() {
    let server = MockServer::start().await;
    let client = create_test_client(&server);

    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(403).set_body_string("API rate limit exceeded for user"),
        )
        .mount(&server)
        .await;

    #[derive(serde::Deserialize, Debug)]
    struct Empty {}

    let result: Result<Empty, _> = client.query("query { viewer { id } }", None).await;

    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("rate limit"), "Error was: {}", err);
}

#[tokio::test]
async fn test_graphql_error_response() {
    let server = MockServer::start().await;
    let client = create_test_client(&server);

    Mock::given(method("POST"))
        .respond_with(graphql_error_response("Could not resolve to a Repository"))
        .mount(&server)
        .await;

    #[derive(serde::Deserialize, Debug)]
    struct Empty {}

    let result: Result<Empty, _> = client.query("query { repository { id } }", None).await;

    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("Could not resolve to a Repository"),
        "Error was: {}",
        err
    );
}

#[tokio::test]
async fn test_server_error() {
    let server = MockServer::start().await;
    let client = create_test_client(&server);

    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500).set_body_string("Internal Server Error"))
        .mount(&server)
        .await;

    #[derive(serde::Deserialize, Debug)]
    struct Empty {}

    let result: Result<Empty, _> = client.query("query { viewer { id } }", None).await;

    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("500"), "Error was: {}", err);
}

// =============================================================================
// Repository ID Tests
// =============================================================================

#[tokio::test]
async fn test_get_repository_id() {
    let server = MockServer::start().await;
    let client = create_test_client(&server);

    Mock::given(method("POST"))
        .respond_with(graphql_response(json!({
            "repository": { "id": "R_kgDOExample" }
        })))
        .mount(&server)
        .await;

    let repo_id = client.get_repository_id("owner", "repo").await.unwrap();
    assert_eq!(repo_id, "R_kgDOExample");
}

#[tokio::test]
async fn test_get_repository_id_not_found() {
    let server = MockServer::start().await;
    let client = create_test_client(&server);

    Mock::given(method("POST"))
        .respond_with(graphql_response(json!({ "repository": null })))
        .mount(&server)
        .await;

    let result = client.get_repository_id("owner", "nonexistent").await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("not found"));
}

// =============================================================================
// Issue Creation Tests
// =============================================================================

#[tokio::test]
async fn test_create_issue() {
    let server = MockServer::start().await;
    let client = create_test_client(&server);

    // Batch create uses aliases like create_0, create_1, etc.
    Mock::given(method("POST"))
        .respond_with(graphql_response(json!({
            "create_0": {
                "issue": {
                    "id": "I_kwDOExample",
                    "number": 42,
                    "url": "https://github.com/owner/repo/issues/42"
                }
            }
        })))
        .mount(&server)
        .await;

    use ticket_to_ride::github::issues::IssueCreate;
    let creates = vec![IssueCreate {
        title: "Test Issue".to_string(),
        body: "Test body".to_string(),
        label_ids: vec![],
        issue_type_id: None,
    }];

    let results = client
        .create_issues_batch("R_123", &creates, None)
        .await
        .unwrap();

    assert_eq!(results.len(), 1);
    let info = results[0].as_ref().unwrap();
    assert_eq!(info.id, "I_kwDOExample");
    assert_eq!(info.number, 42);
}

// =============================================================================
// Batch Operation Tests
// =============================================================================

#[tokio::test]
async fn test_batch_create_multiple_issues() {
    let server = MockServer::start().await;
    let client = create_test_client(&server);

    // The batch creates use aliased mutations like create_0, create_1, etc.
    Mock::given(method("POST"))
        .respond_with(graphql_response(json!({
            "create_0": {
                "issue": {
                    "id": "I_1",
                    "number": 1,
                    "url": "https://github.com/owner/repo/issues/1"
                }
            },
            "create_1": {
                "issue": {
                    "id": "I_2",
                    "number": 2,
                    "url": "https://github.com/owner/repo/issues/2"
                }
            }
        })))
        .mount(&server)
        .await;

    use ticket_to_ride::github::issues::IssueCreate;
    let creates = vec![
        IssueCreate {
            title: "Issue 1".to_string(),
            body: "Body 1".to_string(),
            label_ids: vec![],
            issue_type_id: None,
        },
        IssueCreate {
            title: "Issue 2".to_string(),
            body: "Body 2".to_string(),
            label_ids: vec![],
            issue_type_id: None,
        },
    ];

    let results = client
        .create_issues_batch("R_123", &creates, None)
        .await
        .unwrap();

    assert_eq!(results.len(), 2);
    assert_eq!(results[0].as_ref().unwrap().number, 1);
    assert_eq!(results[1].as_ref().unwrap().number, 2);
}

#[tokio::test]
async fn test_batch_update_issues() {
    let server = MockServer::start().await;
    let client = create_test_client(&server);

    // Batch update needs full issue info in response
    Mock::given(method("POST"))
        .respond_with(graphql_response(json!({
            "update_0": {
                "issue": {
                    "id": "I_1",
                    "number": 1,
                    "url": "https://github.com/owner/repo/issues/1"
                }
            },
            "update_1": {
                "issue": {
                    "id": "I_2",
                    "number": 2,
                    "url": "https://github.com/owner/repo/issues/2"
                }
            }
        })))
        .mount(&server)
        .await;

    use ticket_to_ride::github::issues::IssueUpdate;
    let updates = vec![
        IssueUpdate {
            issue_id: "I_1".to_string(),
            title: "Updated 1".to_string(),
            body: "New body 1".to_string(),
            issue_type_id: None,
        },
        IssueUpdate {
            issue_id: "I_2".to_string(),
            title: "Updated 2".to_string(),
            body: "New body 2".to_string(),
            issue_type_id: None,
        },
    ];

    let results = client.update_issues_batch(&updates).await.unwrap();

    assert_eq!(results.len(), 2);
    assert!(results.get("I_1").unwrap().is_ok());
    assert!(results.get("I_2").unwrap().is_ok());
}

// =============================================================================
// Label Tests
// =============================================================================

#[tokio::test]
async fn test_get_labels() {
    let server = MockServer::start().await;
    let client = create_test_client(&server);

    Mock::given(method("POST"))
        .respond_with(graphql_response(json!({
            "repository": {
                "labels": {
                    "nodes": [
                        { "id": "L_1", "name": "bug" },
                        { "id": "L_2", "name": "enhancement" }
                    ]
                }
            }
        })))
        .mount(&server)
        .await;

    let labels = client.get_labels("owner", "repo").await.unwrap();

    assert_eq!(labels.len(), 2);
    assert_eq!(labels[0].name, "bug");
    assert_eq!(labels[1].name, "enhancement");
}

#[tokio::test]
async fn test_create_label() {
    let server = MockServer::start().await;
    let client = create_test_client(&server);

    // create_label expects LabelInfo with id and name
    Mock::given(method("POST"))
        .respond_with(graphql_response(json!({
            "createLabel": {
                "label": {
                    "id": "L_new",
                    "name": "new-label"
                }
            }
        })))
        .mount(&server)
        .await;

    let label = client
        .create_label("R_123", "new-label", "ff0000")
        .await
        .unwrap();

    assert_eq!(label.id, "L_new");
    assert_eq!(label.name, "new-label");
}

// =============================================================================
// Project Tests
// =============================================================================

#[tokio::test]
async fn test_find_project_by_name() {
    let server = MockServer::start().await;
    let client = create_test_client(&server);

    // First call: repo-level projects (not found)
    // Second call: check if org
    // Third call: user-level projects (found)
    Mock::given(method("POST"))
        .respond_with(graphql_response(json!({
            "repository": {
                "projectsV2": {
                    "nodes": []
                }
            }
        })))
        .up_to_n_times(1)
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .respond_with(graphql_response(json!({
            "repository": {
                "owner": { "__typename": "User" }
            }
        })))
        .up_to_n_times(1)
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .respond_with(graphql_response(json!({
            "user": {
                "projectsV2": {
                    "nodes": [
                        { "id": "PVT_123", "title": "My Project", "number": 1 }
                    ]
                }
            }
        })))
        .mount(&server)
        .await;

    let project = client
        .find_project("owner", "repo", "My Project")
        .await
        .unwrap();

    assert!(project.is_some());
    let p = project.unwrap();
    assert_eq!(p.id, "PVT_123");
    assert_eq!(p.title, "My Project");
    assert_eq!(p.number, 1);
}

#[tokio::test]
async fn test_add_issue_to_project() {
    let server = MockServer::start().await;
    let client = create_test_client(&server);

    Mock::given(method("POST"))
        .respond_with(graphql_response(json!({
            "addProjectV2ItemById": {
                "item": { "id": "PVTI_item123" }
            }
        })))
        .mount(&server)
        .await;

    let result = client
        .add_issue_to_project("PVT_project", "I_issue")
        .await
        .unwrap();

    assert_eq!(result.item_id, "PVTI_item123");
}

// =============================================================================
// Sub-Issue Tests
// =============================================================================

#[tokio::test]
async fn test_add_sub_issues_batch() {
    let server = MockServer::start().await;
    let client = create_test_client(&server);

    // sub-issues batch uses link_0 and expects subIssue in response
    Mock::given(method("POST"))
        .respond_with(graphql_response(json!({
            "link_0": {
                "subIssue": { "id": "I_child" }
            }
        })))
        .mount(&server)
        .await;

    use ticket_to_ride::github::subissues::SubIssueLink;
    let links = vec![SubIssueLink {
        parent_issue_id: "I_parent".to_string(),
        child_issue_id: "I_child".to_string(),
    }];

    let results = client.add_sub_issues_batch(&links).await.unwrap();

    assert_eq!(results.len(), 1);
    assert!(results[0].is_ok());
}
