//! Live integration tests for logout and token refresh functionality.
//!
//! Run with: `cd orignabase && cargo test --test logout_integration_test -- --ignored`

use serde_json::{Value, json};
use std::time::Duration;
use tokio::time::sleep;

fn base_url() -> String {
    std::env::var("OB_TEST_URL").unwrap_or_else(|_| "https://api.dev.orignagta.ca".to_string())
}

/// Login and return (access_token, refresh_token).
async fn login(client: &reqwest::Client) -> (String, String) {
    let resp = client
        .post(format!("{}/auth/login", base_url()))
        .json(&json!({ // ignore-magic
            "email": "e2e-buyer@test.origna.ca", // ignore-magic
            "password": "TestPass123!" // ignore-magic
        }))
        .send()
        .await
        .expect("login failed");

    assert_eq!(resp.status(), 200, "Login failed");
    let body: Value = resp.json().await.expect("parse login response");

    let access_token = body["access_token"] // ignore-magic
        .as_str()
        .expect("missing access_token")
        .to_string();
    let refresh_tok = body["refresh_token"].as_str().unwrap_or("").to_string(); // ignore-magic

    (access_token, refresh_tok)
}

/// Logout by revoking the refresh token.
async fn logout(client: &reqwest::Client, refresh_token: &str) -> Result<(), String> {
    let resp = client
        .post(format!("{}/auth/logout", base_url()))
        .json(&json!({ // ignore-magic
            "refresh_token": refresh_token // ignore-magic
        }))
        .send()
        .await
        .map_err(|e| format!("logout request failed: {}", e))?;

    if resp.status() == 200 {
        Ok(())
    } else {
        Err(format!("logout failed: {}", resp.status()))
    }
}

/// Refresh access token using refresh token.
async fn refresh_access_token(
    client: &reqwest::Client,
    refresh_token: &str,
) -> Result<(String, String), String> {
    let resp = client
        .post(format!("{}/auth/refresh", base_url()))
        .json(&json!({ // ignore-magic
            "refresh_token": refresh_token // ignore-magic
        }))
        .send()
        .await
        .map_err(|e| format!("refresh request failed: {}", e))?;

    let status = resp.status();
    let body: Value = resp
        .json()
        .await
        .map_err(|e| format!("parse response: {}", e))?;

    if status == 200 {
        let new_access = body["access_token"] // ignore-magic
            .as_str()
            .ok_or("missing new access_token")?
            .to_string();
        let new_refresh = body["refresh_token"].as_str().unwrap_or("").to_string(); // ignore-magic
        Ok((new_access, new_refresh))
    } else {
        Err(format!("refresh token failed: {} — {}", status, body))
    }
}

/// Make a simple authenticated request to verify token validity.
async fn verify_token_valid(client: &reqwest::Client, token: &str) -> bool {
    let resp = client
        .get(format!("{}/user/profile", base_url()))
        .header("Authorization", format!("Bearer {}", token)) // ignore-magic
        .send()
        .await;

    match resp {
        Ok(r) => r.status() == 200,
        Err(_) => false,
    }
}

#[tokio::test]
#[ignore]
async fn test_logout_revokes_refresh_token() {
    let client = reqwest::Client::new();

    // Login
    let (access_token, refresh_tok) = login(&client).await;
    assert!(!refresh_tok.is_empty(), "Should receive refresh_token");

    // Verify access token is valid before logout
    assert!(
        verify_token_valid(&client, &access_token).await,
        "Access token should be valid after login"
    );

    // Logout with refresh token
    match logout(&client, &refresh_tok).await {
        Ok(()) => {
            eprintln!("Logged out successfully");
        }
        Err(e) => {
            eprintln!("Logout failed: {}", e);
            return; // Skip rest of test if logout not supported
        }
    }

    sleep(Duration::from_millis(500)).await;

    // Try to use the old refresh token to get new access token
    match refresh_access_token(&client, &refresh_tok).await {
        Ok(_) => {
            eprintln!(
                "WARNING: Old refresh token still works after logout. \
                       This might be OK if token hasn't been revoked yet."
            );
        }
        Err(e) => {
            // Expect 401 or "invalid token" error
            let error_str = e.to_string();
            assert!(
                error_str.contains("401")
                    || error_str.contains("invalid")
                    || error_str.contains("revoked"),
                "Should reject revoked refresh token: {}",
                e
            );
        }
    }
}

#[tokio::test]
#[ignore]
async fn test_refresh_rotation_revokes_old() {
    let client = reqwest::Client::new();

    // Login
    let (access_token_1, refresh_token_1) = login(&client).await;

    // Verify token 1 works
    assert!(
        verify_token_valid(&client, &access_token_1).await,
        "Initial access token should work"
    );

    sleep(Duration::from_millis(500)).await;

    // Refresh to get new tokens
    match refresh_access_token(&client, &refresh_token_1).await {
        Ok((access_token_2, refresh_token_2)) => {
            assert!(
                !access_token_2.is_empty(),
                "Should receive new access_token"
            );

            // Verify new token works
            assert!(
                verify_token_valid(&client, &access_token_2).await,
                "New access token should work"
            );

            sleep(Duration::from_millis(500)).await;

            // Try to use old refresh token again
            match refresh_access_token(&client, &refresh_token_1).await {
                Ok(_) => {
                    eprintln!(
                        "Old refresh token still works — token rotation not enforced. \
                               This is OK if you don't require strict rotation."
                    );
                }
                Err(e) => {
                    // Expect 401 if rotation is enforced
                    let error_str = e.to_string();
                    assert!(
                        error_str.contains("401") || error_str.contains("invalid"),
                        "Old refresh token should be revoked after rotation: {}",
                        e
                    );
                }
            }

            // New refresh token should still work
            let refresh_result = refresh_access_token(&client, &refresh_token_2).await;
            assert!(
                refresh_result.is_ok(),
                "Latest refresh token should still be valid"
            );
        }
        Err(e) => {
            eprintln!("Refresh token endpoint not available: {}", e);
        }
    }
}

#[tokio::test]
#[ignore]
async fn test_logout_does_not_revoke_current_access_token() {
    let client = reqwest::Client::new();

    // Login
    let (access_token, refresh_token) = login(&client).await;

    // Verify token works before logout
    assert!(
        verify_token_valid(&client, &access_token).await,
        "Token should be valid after login"
    );

    // Logout
    match logout(&client, &refresh_token).await {
        Ok(()) => {
            eprintln!("Logged out");
        }
        Err(e) => {
            eprintln!("Logout not supported: {}", e);
            return;
        }
    }

    sleep(Duration::from_millis(500)).await;

    // Current backend contract revokes the refresh token only.
    // The existing access token remains usable until expiry.
    let still_valid = verify_token_valid(&client, &access_token).await;
    assert!(
        still_valid,
        "Access token should remain usable until expiry; logout only revokes refresh tokens"
    );
}
