# Testing Guide for Nucleus

This guide explains how to run tests, understand test coverage, and debug tests in the Nucleus project.

## Running Tests

### Run all tests
```bash
cargo test --workspace
```

### Run specific test suite
```bash
cargo test -p nucleus-core      # Tests for the core engine
cargo test -p nucleus-server    # Tests for the HTTP server
cargo test -p nucleus-ffi       # Tests for the C FFI bindings
```

### Run specific test
```bash
cargo test test_name
cargo test test::create_and_list_domains
```

### Run tests with output
```bash
cargo test -- --nocapture
```

### Run only integration tests
```bash
cargo test -p nucleus-core --test engine_integration
```

### Run tests with verbose output
```bash
cargo test -- --verbose
```

## Test Coverage

### Current Test Coverage

| Component | Unit Tests | Integration Tests | E2E Tests | Total |
|-----------|-----------|-------------------|-----------|-------|
| **core** | 12 | 6 | - | 18 |
| **server** | - | - | 16 | 16 |
| **ffi** | - | - | 0 | 0 |
| **Total** | 12 | 6 | 16 | 34* |

*Note: The 12 unit tests in core include tests in multiple modules

### Test Files

**Core Tests:**
- `crates/core/tests/engine_integration.rs` - End-to-end tests for engine
- Unit tests in each module (`crates/core/src/*/mod.rs`)

**Server Tests:**
- `crates/server/src/routes.rs` - HTTP API E2E tests

**FFI Tests:**
- No tests in `crates/ffi/` (will be added)

## Writing Tests

### Unit Tests

Unit tests are located in each module:

```rust
// crates/core/src/auth.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_creation() {
        let token = ApiToken::new("test", vec![]);
        assert_eq!(token.name, "test");
    }

    #[test]
    fn test_scope_permissions() {
        let scope = Scope::for_domain(domain_id, Perm::Read);
        assert!(scope.allows(domain_id, Perm::Read));
        assert!(!scope.allows(domain_id, Perm::Write));
    }
}
```

### Integration Tests

Integration tests are in `crates/core/tests/`:

```rust
// crates/core/tests/engine_integration.rs
use std::collections::BTreeMap;
use nucleus_core::embed::MockEmbedder;
use nucleus_core::engine::SearchRequest;
use nucleus_core::id::DomainId;
use nucleus_core::storage::Storage;
use nucleus_core::Engine;

fn fresh_engine() -> (Engine, TempDir) {
    let dir = TempDir::new().expect("create temp dir");
    let keyfile = dir.path().join("test.key");
    let storage = Storage::open_with_options(
        dir.path().join("test.redb"),
        None,
        Some(&keyfile)
    ).expect("open storage");

    let embedder: Arc<dyn nucleus_core::embed::Embedder> =
        Arc::new(MockEmbedder::new(64));

    let engine = Engine::open(
        storage,
        embedder,
        IndexKind::Flat,
        None,
    ).expect("open engine");

    (engine, dir)
}

#[test]
fn test_create_domain() {
    let (engine, _dir) = fresh_engine();
    let domain = engine.create_domain("test", None).expect("create domain");

    assert_eq!(domain.name, "test");
    assert!(domain.id > 0);
}

#[test]
fn test_search() {
    let (engine, _dir) = fresh_engine();
    let domain = engine.create_domain("test", None).expect("create domain");

    // Ingest a document
    let meta = BTreeMap::new();
    engine.ingest_document(
        domain.id,
        "test.doc",
        Some("test.doc".to_string()),
        meta,
        vec![],
        IngestBody::Text("This is a test document about Nucleus.".to_string()),
    ).expect("ingest");

    // Search
    let results = engine.search(
        domain.id,
        SearchRequest {
            query: QueryInput::Text("Nucleus database".to_string()),
            k: 10,
            tags: vec![],
            match_all: false,
            document_ids: vec![],
            subdomain: None,
            filter: None,
            diversity: 0.0,
        },
    ).expect("search");

    assert!(!results.is_empty());
}
```

### Using MockEmbedder

Use `MockEmbedder` for deterministic tests without downloading ONNX models:

```rust
use nucleus_core::embed::MockEmbedder;

let embedder = Arc::new(MockEmbedder::new(64));  // Use dimension 64
let engine = Engine::open(storage, embedder, IndexKind::Flat, None)?;
```

### Using TempFile for Isolated Databases

```rust
use tempfile::TempDir;

let dir = TempDir::new().expect("create temp dir");
let keyfile = dir.path().join("test.key");
let storage = Storage::open_with_options(
    dir.path().join("test.redb"),
    None,
    Some(&keyfile)
).expect("open storage");
```

## Testing Components

### Testing the Engine

The engine tests use `fresh_engine()` helper for isolated databases:

```rust
fn fresh_engine() -> (Engine, TempDir) {
    let dir = TempDir::new().expect("create temp dir");
    let keyfile = dir.path().join("test.key");
    let storage = Storage::open_with_options(
        dir.path().join("test.redb"),
        None,
        Some(&keyfile)
    ).expect("open storage");

    let embedder: Arc<dyn nucleus_core::embed::Embedder> =
        Arc::new(MockEmbedder::new(64));

    let engine = Engine::open(
        storage,
        embedder,
        IndexKind::Flat,
        None,
    ).expect("open engine");

    (engine, dir)
}
```

### Testing Storage

```rust
#[test]
fn test_storage_crud() {
    let dir = TempDir::new().expect("create temp dir");
    let storage = Storage::open_with_options(
        dir.path().join("test.redb"),
        None,
        None,
    ).expect("open storage");

    // Create domain
    let domain_id = storage.create_domain("test")?;

    // Get domain
    let domain = storage.get_domain(domain_id)?;
    assert_eq!(domain.name, "test");

    // Delete domain
    storage.delete_domain(domain_id)?;
}
```

### Testing Auth

```rust
#[test]
fn test_token_scopes() {
    use nucleus_core::auth::{Scope, Perm, DomainScope};

    // Create token with read scope on specific domain
    let token = ApiToken::new(
        "test",
        vec![Scope {
            domain: DomainScope::One(domain_id),
            perm: Perm::Read,
        }],
    );

    // Verify token allows read access
    assert!(token.allows(domain_id, Perm::Read));

    // Verify token does not allow write access
    assert!(!token.allows(domain_id, Perm::Write));
}
```

## Debugging Tests

### Enable Debug Logs

```bash
RUST_LOG=debug cargo test -- --nocapture
RUST_LOG=nucleus_core::engine=trace cargo test
```

### Running Specific Tests with Logs

```bash
RUST_LOG=debug cargo test test::create_domain -- --nocapture
```

### Viewing Test Output

```bash
cargo test -- --nocapture 2>&1 | less
```

### Profiling Tests

```bash
time cargo test -p nucleus-core
```

### Checking Test Isolation

```bash
cargo test -- --test-threads=1  # Run tests sequentially
```

## Test Performance

### Running Benchmarks

```bash
cargo bench --workspace
cargo bench --bench search
```

### View Benchmark Results

```bash
# HTML reports
open target/criterion/

# Text output
cargo bench -- --nocapture
```

### Compare Benchmarks

```bash
cargo bench --profile-time 5m
```

## CI Integration

Tests are run in GitHub Actions:

### Workflow: `.github/workflows/ci.yml`

```yaml
test:
  runs-on: ${{ matrix.os }}
  strategy:
    matrix:
      os: [ubuntu-latest, windows-latest, macos-latest]
  steps:
    - uses: actions/checkout@v4
    - uses: dtolnay/rust-toolchain@stable
    - uses: Swatinem/rust-cache@v2
    - name: Test
      run: cargo test --workspace
```

### Test Coverage Report

Coverage is not currently measured. To add coverage:

```toml
# Cargo.toml
[dev-dependencies]
cargo-tarpaulin = "0.23"

# .github/workflows/ci.yml
- name: Coverage
  run: cargo tarpaulin --out Html
```

## Best Practices

### 1. Use Isolated Test Data

```rust
fn fresh_engine() -> (Engine, TempDir) {
    let dir = TempDir::new().expect("create temp dir");
    // Use TempDir for isolated database
    ...
}
```

### 2. Use MockEmbedder for Embeddings

```rust
use nucleus_core::embed::MockEmbedder;

let embedder = Arc::new(MockEmbedder::new(64));
```

### 3. Test Edge Cases

```rust
#[test]
fn test_empty_result() {
    let results = engine.search(...);
    assert!(results.is_empty());
}

#[test]
fn test_k_zero() {
    let results = engine.search(..., k: 0);
    assert!(results.is_empty());
}
```

### 4. Test Error Paths

```rust
#[test]
fn test_invalid_domain() {
    let result = engine.search(invalid_domain, ...);
    assert!(matches!(result, Err(NucleusError::NotFound(_))));
}
```

### 5. Document Test Purpose

```rust
/// Tests that ingestion correctly handles document deduplication
#[test]
fn test_ingest_duplicate() {
    ...
}
```

## Contributing Tests

When adding new features, also add tests:

1. **Unit tests** for new functions
2. **Integration tests** for end-to-end flows
3. **Edge case tests** for error conditions
4. **Performance tests** for benchmarks

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_feature() {
        // Test implementation
    }

    #[test]
    fn test_new_feature_error() {
        // Test error handling
    }
}
```

## Troubleshooting

### Tests Fail on CI but Pass Locally

1. Check environment variables
2. Check platform differences (Windows vs Linux)
3. Check thread count: `RUST_TEST_THREADS=1 cargo test`

### Tests Time Out

1. Increase timeout: `cargo test -- --test-timeout=120`

### Tests Leak Memory

1. Check for static variables
2. Check for file handles
3. Use `TempDir` to clean up files

### Tests Fail with DB Errors

1. Ensure `fresh_engine()` is used for isolated tests
2. Check file permissions
3. Verify TempDir is dropped after test

## Resources

- [Rust Testing Guide](https://doc.rust-lang.org/book/ch11-00-testing.html)
- [Criterion Documentation](https://bheisler.github.io/criterion.rs/)
- [Testing Best Practices](https://doc.rust-lang.org/rust-by-example/testing.html)
