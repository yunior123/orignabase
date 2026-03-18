# OrignaBase Testing Rules

## Always loaded

- Unit tests: `#[tokio::test]` in same file as code (mod tests block)
- Integration tests: `#[tokio::test]` + `#[ignore]` in `crates/orignabase/tests/`
- Integration pattern: `register_test_user()` → `make_request()` → assert status
- Never commit tests that pass without real server (`#[ignore]` is mandatory)
- Run before commit: `cargo test` (unit only, fast)
- Run before deploy: `cargo test -- --ignored` (needs OB_TEST_URL set)
- Test coverage: `cargo llvm-cov` (not tarpaulin)
