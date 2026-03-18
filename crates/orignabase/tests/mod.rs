//! Integration tests for OrignaBase repositories and handlers.
//!
//! These tests require a running OrignaBase instance.
//! Run with: cargo test --test <name> -- --ignored
//!
//! To start the server:
//!   surreal start --user root --pass root memory
//!   cargo run -- serve
//!
//! Set OB_TEST_URL to override the default (http://localhost:8080).

// Auth tests
pub mod auth_repository_test;

// User tests
pub mod user_repository_test;

// Product tests
pub mod product_repository_test;

// Cart tests
pub mod cart_repository_test;

// Order tests
pub mod order_repository_test;
