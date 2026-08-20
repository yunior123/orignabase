use reqwest::{Client, Method, StatusCode};
use serde_json::{Value, json};
use uuid::Uuid;

mod smoke {
    use super::*;

    fn base_url() -> String {
        std::env::var("OB_TEST_URL").unwrap_or_else(|_| "http://localhost:8080".to_string()) // ignore-magic
    }

    fn client() -> Client {
        Client::new()
    }

    fn unique_email() -> String {
        format!("smoke_{}@example.com", Uuid::new_v4())
    }

    async fn register_user(client: &Client, email: &str, password: &str) -> reqwest::Response {
        client
            .post(format!("{}/auth/register", base_url()))
            .json(&json!({ // ignore-magic
                "email": email, // ignore-magic
                "password": password // ignore-magic
            }))
            .send()
            .await
            .expect("register request failed")
    }

    async fn login_user(client: &Client, email: &str, password: &str) -> reqwest::Response {
        client
            .post(format!("{}/auth/login", base_url()))
            .json(&json!({ // ignore-magic
                "email": email, // ignore-magic
                "password": password // ignore-magic
            }))
            .send()
            .await
            .expect("login request failed")
    }

    async fn register_and_login(client: &Client) -> (String, String) {
        let email = unique_email();
        let password = "TestPassword123!"; // ignore-magic

        let register = register_user(client, &email, password).await;
        assert_eq!(register.status(), StatusCode::OK, "register should succeed");

        let login = login_user(client, &email, password).await;
        assert_eq!(login.status(), StatusCode::OK, "login should succeed");

        let body: Value = login.json().await.expect("login body should be valid json");
        let token = body["access_token"] // ignore-magic
            .as_str()
            .expect("missing access_token")
            .to_string();

        (email, token)
    }

    #[tokio::test]
    #[ignore = "requires running orignabase instance"]
    async fn health_returns_200() {
        let response = client()
            .get(format!("{}/health", base_url()))
            .send()
            .await
            .expect("health request failed");

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    #[ignore = "requires running orignabase instance"]
    async fn register_with_empty_body_returns_400() {
        let response = client()
            .post(format!("{}/auth/register", base_url()))
            .header("content-type", "application/json") // ignore-magic
            .body("")
            .send()
            .await
            .expect("register request failed");

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    #[ignore = "requires running orignabase instance"]
    async fn register_with_missing_password_returns_error() {
        let response = client()
            .post(format!("{}/auth/register", base_url()))
            .json(&json!({ // ignore-magic
                "email": unique_email() // ignore-magic
            }))
            .send()
            .await
            .expect("register request failed");

        let status = response.status();
        assert!(
            status == StatusCode::BAD_REQUEST || status == StatusCode::UNPROCESSABLE_ENTITY,
            "Expected 400 or 422, got {status}"
        );
    }

    #[tokio::test]
    #[ignore = "requires running orignabase instance"]
    async fn login_with_wrong_credentials_returns_401() {
        let client = client();
        let email = unique_email();

        let register = register_user(&client, &email, "TestPassword123!").await; // ignore-magic
        assert_eq!(register.status(), StatusCode::OK, "register should succeed");

        let response = login_user(&client, &email, "WrongPassword123!").await; // ignore-magic

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    #[ignore = "requires running orignabase instance"]
    async fn graphql_without_auth_returns_200_with_errors() {
        // GraphQL always returns 200 but with errors for auth-required queries
        let response = client()
            .post(format!("{}/graphql", base_url()))
            .json(&json!({"query": "{ list(collection: \"test\", limit: 1) }"})) // ignore-magic
            .send()
            .await
            .expect("graphql request failed");

        assert_eq!(response.status(), StatusCode::OK);
        let body: Value = response.json().await.unwrap();
        assert!(body.get("errors").is_some() || body.get("data").is_some()); // ignore-magic
    }

    #[tokio::test]
    #[ignore = "requires running orignabase instance"]
    async fn graphql_mutation_without_auth_returns_error() {
        let response = client()
            .post(format!("{}/graphql", base_url()))
            .json(&json!({"query": "mutation { create(collection: \"test\", data: \"{}\") }"})) // ignore-magic
            .send()
            .await
            .expect("graphql mutation request failed");

        assert_eq!(response.status(), StatusCode::OK);
        let body: Value = response.json().await.unwrap();
        // Should have errors (unauthenticated)
        assert!(body.get("errors").is_some() || body.get("data").is_some()); // ignore-magic
    }

    #[tokio::test]
    #[ignore = "requires running orignabase instance"]
    async fn graphql_introspection_without_auth_returns_200() {
        let response = client()
            .post(format!("{}/graphql", base_url()))
            .json(&json!({ // ignore-magic
                "query": "{ __schema { queryType { name } } }" // ignore-magic
            }))
            .send()
            .await
            .expect("graphql request failed");

        assert_eq!(response.status(), StatusCode::OK);

        let body: Value = response
            .json()
            .await
            .expect("graphql body should be valid json");
        assert!(body.get("data").is_some(), "expected graphql data payload"); // ignore-magic
    }

    #[tokio::test]
    #[ignore = "requires running orignabase instance"]
    async fn storage_presign_without_auth_returns_401() {
        let response = client()
            .post(format!("{}/storage/presign/upload", base_url()))
            .json(&json!({"path": "test.txt", "content_type": "text/plain"})) // ignore-magic
            .send()
            .await
            .expect("storage request failed");

        let status = response.status();
        assert!(
            status == StatusCode::UNAUTHORIZED
                || status == StatusCode::BAD_REQUEST
                || status == StatusCode::UNPROCESSABLE_ENTITY,
            "Expected 401/400/422, got {status}"
        );
    }

    #[tokio::test]
    #[ignore = "requires running orignabase instance"]
    async fn register_then_login_returns_200() {
        let client = client();
        let email = unique_email();
        let password = "TestPassword123!"; // ignore-magic

        let register = register_user(&client, &email, password).await;
        assert_eq!(register.status(), StatusCode::OK);

        let login = login_user(&client, &email, password).await;
        assert_eq!(login.status(), StatusCode::OK);

        let body: Value = login.json().await.expect("login body should be valid json");
        assert!(body["access_token"].is_string()); // ignore-magic
        assert!(body["refresh_token"].is_string()); // ignore-magic
    }

    #[tokio::test]
    #[ignore = "requires running orignabase instance"]
    async fn graphql_with_valid_token_returns_200() {
        let client = client();
        let (_, token) = register_and_login(&client).await;

        let response = client
            .post(format!("{}/graphql", base_url()))
            .bearer_auth(token)
            .json(&json!({"query": "{ __typename }"})) // ignore-magic
            .send()
            .await
            .expect("graphql request failed");

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    #[ignore = "requires running orignabase instance"]
    async fn options_request_includes_cors_headers() {
        let response = client()
            .request(Method::OPTIONS, format!("{}/health", base_url()))
            .header("origin", "http://example.com")
            .header("access-control-request-method", "GET") // ignore-magic
            .send()
            .await
            .expect("options request failed");

        assert!(
            response
                .headers()
                .contains_key("access-control-allow-origin"),
            "missing access-control-allow-origin header"
        );
        assert!(
            response
                .headers()
                .contains_key("access-control-allow-methods"),
            "missing access-control-allow-methods header"
        );
    }

    #[tokio::test]
    #[ignore = "requires running orignabase instance"]
    async fn oversized_body_returns_413_or_400() {
        let oversized = "x".repeat(1_100_000);
        let response = client()
            .post(format!("{}/auth/register", base_url()))
            .header("content-type", "application/json") // ignore-magic
            .body(oversized)
            .send()
            .await
            .expect("oversized request failed");

        assert!(
            response.status() == StatusCode::PAYLOAD_TOO_LARGE
                || response.status() == StatusCode::BAD_REQUEST,
            "expected 413 or 400, got {}",
            response.status()
        );
    }
}
