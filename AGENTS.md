# AGENTS.md - Agentic Coding Guidelines for Flicker

## Build Commands

```bash
# Build (release)
cargo build --release

# Build (debug)
cargo build

# Run all tests
cargo test --verbose

# Run a single test
cargo test test_name_here

# Check formatting
cargo fmt -- --check

# Format code
cargo fmt

# Run clippy (linting)
cargo clippy -- -D warnings
```

## Project Structure

- **Language**: Rust (Edition 2024)
- **Type**: Async CLI application (log shipper)
- **Runtime**: Tokio (full features)
- **Main entry**: `src/main.rs`
- **Source modules**: `src/*.rs` (tailer, filter, registry, destinations/, etc.)
- **Config**: YAML-based (`flicker.yaml`)

## Code Style Guidelines

### Imports
- Group imports: stdlib → external crates → internal modules
- Use `use crate::module::Item` for internal imports
- External crates: `tokio`, `serde`, `anyhow`, `async-trait`, `clap`, `regex`, etc.

### Error Handling
- Use `anyhow::Result<()>` for functions that can fail
- Use `?` operator for error propagation
- Use `anyhow::anyhow!("message")` for custom errors
- User-facing errors go to stderr: `eprintln!("Error: {}", e)`
- Info/logging goes to stdout: `println!("Message")`

### Types & Naming
- **Structs/Enums**: PascalCase (`LogEntry`, `DestinationConfig`)
- **Functions/variables**: snake_case (`send_batch`, `buffer_size`)
- **Constants**: SCREAMING_SNAKE_CASE or plain uppercase
- **Traits**: PascalCase with clear action names (`Destination`)
- **Async functions**: Same naming, marked with `async`
- **Generic params**: Single uppercase letters (`T`, `K`, `V`)

### Formatting
- Run `cargo fmt` before committing
- 4 spaces for indentation (standard Rust)
- Max line length: ~100 chars (rustfmt default)
- Trailing commas in multi-line structs/enums

### Comments
- Use `//` for single-line comments
- Use `///` for doc comments on public items
- Explain design decisions with: `// DESIGN CHOICE: explanation`
- Keep comments concise and explain "why", not "what"

### Testing
- Tests live in `#[cfg(test)] mod tests` at file bottom
- Use `tempfile` crate for temporary files in tests
- Test naming: `test_<description>` (e.g., `test_tailer_reads_new_lines`)
- Use `assert!`, `assert_eq!`, `assert!(result.is_err())`

### Async Code
- Mark traits with `#[async_trait]` when they have async methods
- Use `tokio::spawn` for concurrent tasks
- Use `tokio::sync::mpsc` for channels
- Prefer `async fn` over callback-based patterns

### Design Patterns
- **Traits**: Use for destination abstraction (`Destination` trait)
- **Factory pattern**: `create_destination()` for instantiating types
- **Error propagation**: `?` operator with `anyhow`
- **Config structs**: Derive `Debug, Deserialize, Clone`
- **Default impls**: Use `serde(default)` and `Default` trait

### Dependencies
- Prefer mature crates: `tokio`, `serde`, `anyhow`, `reqwest`, `clap`
- Check `Cargo.toml` before adding new dependencies
- Feature flags: Use specific features, avoid unused defaults

## CI/CD Requirements

The CI checks (see `.github/workflows/ci.yml`):
1. Tests pass on Linux, macOS, Windows
2. `cargo fmt -- --check` passes
3. `cargo clippy -- -D warnings` passes
4. `cargo build --release` succeeds

Always run these locally before pushing.

## Common Tasks

### Adding a new destination
1. Create module in `src/destinations/<name>.rs`
2. Implement `Destination` trait
3. Add to `src/destinations/mod.rs` factory function
4. Update config parsing if needed

### Adding config options
1. Add field to config struct in `src/config.rs`
2. Use `#[serde(default = "fn_name")]` for defaults
3. Add unit tests in config tests module
4. Update example YAML configs

### Adding tests
1. Add `#[cfg(test)] mod tests` at file bottom if not exists
2. Write descriptive test names
3. Use `tempfile::NamedTempFile` for file tests
4. Run with `cargo test test_name`
