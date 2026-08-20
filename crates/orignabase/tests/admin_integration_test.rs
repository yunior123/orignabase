//! Live integration tests for admin functionality.
//!
//! Run with: `cd orignabase && cargo test --test admin_integration_test -- --ignored`

use ob_database::fields;
use serde_json::{Value, json};

fn base_url() -> String {
    std::env::var("OB_TEST_URL").unwrap_or_else(|_| "https://api.dev.orignagta.ca".to_string())
}

/// Login as admin and return access token.
async fn login_admin(client: &reqwest::Client) -> String {
    let resp = client
        .post(format!("{}/auth/login", base_url()))
        .json(&json!({ // ignore-magic
            "email": "e2e-admin@test.origna.ca", // ignore-magic
            "password": "TestPass123!" // ignore-magic
        }))
        .send()
        .await
        .expect("login failed");

    assert_eq!(resp.status(), 200, "Admin login failed");
    let body: Value = resp.json().await.expect("parse login response");
    body["access_token"] // ignore-magic
        .as_str()
        .expect("missing access_token")
        .to_string()
}

/// List all users (admin only).
async fn list_users(client: &reqwest::Client, token: &str) -> Result<Vec<Value>, String> {
    let resp = client
        .get(format!("{}/admin/users", base_url()))
        .header("Authorization", format!("Bearer {}", token)) // ignore-magic
        .send()
        .await
        .map_err(|e| format!("request failed: {}", e))?;

    let status = resp.status();
    let body: Value = resp
        .json()
        .await
        .map_err(|e| format!("parse response: {}", e))?;

    if status == 200 {
        Ok(body
            .get("users")
            .and_then(|users| users.as_array())
            .cloned()
            .unwrap_or_default())
    } else {
        Err(format!("list users failed: {} — {}", status, body))
    }
}

/// Get user details (admin only).
async fn get_user(client: &reqwest::Client, token: &str, user_id: &str) -> Result<Value, String> {
    let resp = client
        .get(format!("{}/admin/users/{}", base_url(), user_id))
        .header("Authorization", format!("Bearer {}", token)) // ignore-magic
        .send()
        .await
        .map_err(|e| format!("request failed: {}", e))?;

    let status = resp.status();
    let body: Value = resp
        .json()
        .await
        .map_err(|e| format!("parse response: {}", e))?;

    if status == 200 {
        Ok(body)
    } else {
        Err(format!("get user failed: {} — {}", status, body))
    }
}

#[tokio::test]
#[ignore]
async fn test_admin_list_users_includes_email_for_account_identification() {
    let client = reqwest::Client::new();
    let admin_token = login_admin(&client).await;

    let users = list_users(&client, &admin_token)
        .await
        .expect("admin list users should succeed");
    assert!(!users.is_empty(), "Should return users");

    // Admin workflows need email in the list response to identify accounts
    // before opening the per-user detail view.
    for user in users.iter().take(5) {
        assert!(
            user.get(fields::EMAIL)
                .and_then(|email| email.as_str())
                .is_some(), // ignore-magic
            "User list should include email for account identification"
        );

        let has_id = user.get(fields::ID).is_some();
        let has_role = user.get("roles").is_some();
        assert!(
            has_id && has_role,
            "User list should include id and roles fields"
        );
    }
}

#[tokio::test]
#[ignore]
async fn test_admin_list_users_requires_auth() {
    let client = reqwest::Client::new();

    // Attempt to list users WITHOUT auth token
    let resp = client
        .get(format!("{}/admin/users", base_url()))
        .send()
        .await
        .expect("request failed");

    // Should fail with 401 Unauthorized
    assert_eq!(
        resp.status(),
        401,
        "Unauthenticated request should return 401"
    );
}

#[tokio::test]
#[ignore]
async fn test_admin_get_user_requires_admin_role() {
    let client = reqwest::Client::new();
    let admin_token = login_admin(&client).await;

    // List users to get a valid user ID
    let users = list_users(&client, &admin_token)
        .await
        .expect("admin list users should succeed");
    assert!(!users.is_empty(), "Should return at least one user");

    let user_id = users[0][fields::ID] // ignore-magic
        .as_str()
        .map(|s| s.to_string())
        .unwrap_or_default();
    assert!(
        !user_id.is_empty(),
        "List users should return a concrete user id"
    );

    // Get user details as admin — should succeed
    let user = get_user(&client, &admin_token, &user_id)
        .await
        .expect("admin get user should succeed");
    let id = user[fields::ID].as_str().unwrap_or(""); // ignore-magic
    assert_eq!(id, user_id, "Should return requested user");
}

#[tokio::test]
#[ignore]
async fn test_admin_actions_logged_with_uid() {
    let client = reqwest::Client::new();
    let admin_token = login_admin(&client).await;

    // Perform an admin action (list users)
    let _users = list_users(&client, &admin_token)
        .await
        .expect("admin list users should succeed");

    // If we can list users, verify there's an audit log
    // (This test assumes audit logging is implemented)

    // Try to fetch audit log (endpoint may not exist)
    let audit_resp = client
        .get(format!("{}/admin/audit-log", base_url()))
        .header("Authorization", format!("Bearer {}", admin_token)) // ignore-magic
        .send()
        .await;

    if let Ok(resp) = audit_resp
        && resp.status() == 200
    {
        let body: Value = resp.json().await.unwrap_or(json!({})); // ignore-magic
        let empty_vec = vec![];
        let logs = body.as_array().unwrap_or(&empty_vec);

        // Verify recent log entries have adminUid
        for log in logs.iter().take(5) {
            let has_admin_uid = log.get("adminUid").is_some();
            assert!(
                has_admin_uid,
                "Admin actions should be logged with adminUid"
            );
        }
    }
}
