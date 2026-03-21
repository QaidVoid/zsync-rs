# AGENTS.md

Guidelines for AI agents working on the zsync-rs project.

## Project Overview

zsync-rs is a Rust library implementation of zsync - a file transfer tool that allows efficient downloading of partial files over HTTP. This project aims to provide both a library for use in other Rust projects and a command-line tool.

## Development Workflow

### Commits

- **Commit in small chunks** - One logical change per commit
- **Never commit broken state** - All code must compile and pass tests
- **Format before commit** - Run `cargo fmt` before every commit
- **Fix clippy issues** - Run `cargo clippy` and address all warnings before committing

### Commit Messages

Follow conventional commit format with imperative mood:

```
type: message
```

Types:
- `feat:` - New feature
- `fix:` - Bug fix
- `refactor:` - Code refactoring
- `test:` - Adding or updating tests
- `docs:` - Documentation changes
- `chore:` - Maintenance tasks
- `perf:` - Performance improvements
- `style:` - Code style changes (formatting, etc.)
- `ci:` - CI/CD configuration changes

Examples:
- `feat: add zsync control file parser`
- `fix: handle HTTP connection timeouts correctly`
- `refactor: extract block matching logic into separate module`

## Code Quality

### Formatting

```bash
cargo fmt
```

Always run before committing.

### Linting

```bash
cargo clippy --all-targets --all-features -- -D warnings
```

All clippy warnings must be addressed before committing.

### Testing

```bash
cargo test --all-features
```

All tests must pass before committing.

### Fuzzing

This project uses fuzzing to ensure robustness. Fuzz targets are located in `fuzz/` directory.

```bash
cargo fuzz run <target>
```

Add fuzz targets for any parsing or data processing code.

## Pre-commit Checklist

Before every commit, ensure:

1. [ ] `cargo fmt` - Code is formatted
2. [ ] `cargo clippy` - No warnings
3. [ ] `cargo test` - All tests pass
4. [ ] `cargo build` - Clean build with no errors

## Project Structure

```
zsync-rs/
├── src/
│   ├── lib.rs          # Library entry point
│   ├── bin/            # Binary (CLI) entry point
│   └── ...             # Library modules
├── tests/              # Integration tests
├── fuzz/               # Fuzzing targets
├── examples/           # Usage examples
└── benches/            # Performance benchmarks
```

## Library Design

- Library-first approach: core functionality in `src/lib.rs` and modules
- CLI tool uses the library (no duplicated logic)
- Public API should be well-documented with rustdoc
- Use `thiserror` for error types
- Prefer synchronous APIs (no async runtime needed)

## Dependencies

Keep dependencies minimal. Prefer lightweight libraries:
- `ureq` - HTTP client (synchronous, lightweight)
- `thiserror` - Error handling
- `sha2`, `md5`, etc. - Checksum algorithms
- `clap` - CLI argument parsing

Avoid adding libraries for things that can be implemented in a few dozen lines.

## Additional Notes

- Target MSRV (Minimum Supported Rust Version): Latest stable
- Use `#[deny(missing_docs)]` for public APIs
- Prefer `Result<T, E>` over `Option<T>` for fallible operations with context
