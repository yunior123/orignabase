# OrignaBase Code Style Rules

## Always loaded

- Match existing Rust patterns: `axum` handlers, `AppError` type, `tracing::*` for logging
- Error handling: return `Result<Json<T>, AppError>` — never `unwrap()` in handlers
- Use `async-graphql` macros: `#[Object]`, `#[InputObject]`, `#[SimpleObject]`
- Imports: group std → external crates → crate-local (follow existing files)
- No `println!` — use `tracing::info!` / `tracing::error!` / `tracing::debug!`
- Variable names: snake_case, descriptive (no `x`, `tmp`, `data` as catch-alls)
