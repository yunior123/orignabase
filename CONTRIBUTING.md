# Contributing to OrignaBase

## Getting Started

1. Fork the repository
2. Clone your fork
3. Install Rust 1.85+ and SurrealDB v2
4. Run `cargo test --workspace` to verify setup
5. Create a branch for your changes

## Development

```bash
# Run tests
cargo test --workspace

# Run clippy
cargo clippy --workspace --all-targets

# Format code
cargo fmt --all

# Run the server (requires SurrealDB on localhost:8000)
cargo run -- serve
```

## Pull Requests

- Keep PRs focused on a single change
- Add tests for new functionality
- Ensure `cargo clippy` and `cargo fmt` pass
- Update documentation if needed

## Code of Conduct

Be respectful and constructive. We're all here to build something great.
