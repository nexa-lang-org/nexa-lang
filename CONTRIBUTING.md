# Contributing to Nexa

Thank you for your interest in contributing! This document covers everything you need to get started.

## Table of Contents

- [Development Setup](#development-setup)
- [Project Structure](#project-structure)
- [Running Tests](#running-tests)
- [Making Changes](#making-changes)
- [Pull Request Process](#pull-request-process)
- [Code Style](#code-style)

---

## Development Setup

### Prerequisites

- [Rust](https://rustup.rs/) 1.75+
- [Docker](https://docs.docker.com/get-docker/) (for the registry database)
- Git

### Clone and build

```bash
git clone https://github.com/nassime/Nexa-lang.git
cd Nexa-lang
cargo build
```

### Install the CLI locally

```bash
cargo install --path crates/cli
```

### Run the registry (dev)

The registry requires a PostgreSQL database. Start it with Docker Compose:

```bash
docker compose up -d registry-db
```

Then run the registry server:

```bash
DATABASE_URL=postgres://nexa:nexa@localhost:5432/nexa_registry \
JWT_SECRET=dev-secret \
cargo run -p nexa-registry
```

The registry listens on port 4000. Migrations run automatically at startup.

Alternatively, run both the database and the registry together:

```bash
docker compose up
```

---

## Project Structure

```
crates/
├── compiler/   Core language implementation (Clean Architecture)
│   └── src/
│       ├── domain/          AST node types, Span
│       ├── application/
│       │   ├── ports/       SourceProvider trait
│       │   └── services/    Lexer, Parser, Resolver, SemanticAnalyzer,
│       │                    Optimizer, Packager, CodeGenerator
│       └── infrastructure/  FsSourceProvider, MemSourceProvider (tests)
│
├── cli/        Command-line interface
│   └── src/
│       ├── application/
│       │   ├── commands.rs      run, build, package, login, register, publish, install
│       │   ├── credentials.rs   ~/.nexa/credentials.json read/write
│       │   └── project.rs       project.json / nexa-compiler.yaml loading
│       └── interfaces/
│           └── cli.rs           Clap Commands definition
│
├── server/     Axum dev server (HMR via WebSocket)
│
└── registry/   Package registry HTTP API (Clean Architecture)
    └── src/
        ├── domain/          User, Package, PackageVersion entities
        ├── application/
        │   ├── ports/       UserStore + PackageStore traits
        │   └── services/    AuthService (JWT), PackagesService (publish/fetch)
        ├── infrastructure/  PgUserStore, PgPackageStore (sqlx, no macros)
        └── interfaces/
            └── http.rs      Axum router + all handlers
```

---

## Running Tests

```bash
# All tests
cargo test

# Compiler only
cargo test -p nexa-compiler

# CLI only
cargo test -p nexa

# Registry only
cargo test -p nexa-registry
```

To test against the example project manually:

```bash
cargo run --bin nexa -- build --project examples/
```

### End-to-end registry smoke test

```bash
# 1. Start the DB and registry
docker compose up -d

# 2. Register a new account
nexa register --registry http://localhost:4000

# 3. Log in
nexa login --registry http://localhost:4000

# 4. Publish a package
nexa publish --project examples/

# 5. Install a package in another project
nexa install my-lib --project /tmp/test-project
```

---

## Making Changes

### Compiler changes

- **New syntax** — update `domain/ast.rs` (nodes) → `application/services/lexer.rs` (tokens) → `parser.rs` (grammar) → `semantic.rs` (validation) → `codegen.rs` (output)
- **New built-in** — add the primitive to `codegen.rs` `RUNTIME` constant and update the parser
- **New error variant** — add to the relevant error enum, with a clear human-readable message
- **New optimizer pass** — add in `application/services/optimizer.rs`, call it in `optimize()`
- **Bundle format changes** — update `application/services/packager.rs` and bump `NXB_MAGIC`

### CLI changes

- **New command** — add a variant to `Commands` in `interfaces/cli.rs`, implement the handler in `application/commands.rs`
- **Project config fields** — add to `ProjectConfig` or `CompilerConfig` in `application/project.rs` and update tests
- **Registry config** — private registries are declared under `private_registries` in `nexa-compiler.yaml`

### Registry changes

- **New endpoint** — add a handler in `interfaces/http.rs` and register the route in `build_router()`
- **New DB operation** — add a method to the `UserStore` or `PackageStore` trait in `application/ports/storage.rs`, then implement it in `infrastructure/postgres.rs` using `sqlx::query_as::<_, RowType>()` (no compile-time macros)
- **Schema changes** — add a new numbered migration file in `crates/registry/migrations/`

### Tests

Every non-trivial change should include tests:
- Compiler logic → inline `#[cfg(test)]` in the relevant service file
- Optimizer passes → `crates/compiler/src/application/services/optimizer.rs`
- Packager → `crates/compiler/src/application/services/packager.rs`
- Project loading → `crates/cli/src/application/project.rs`
- Auth / packages service → `crates/registry/src/application/services/`

---

## Pull Request Process

1. **Fork** the repository and create a branch from `main`
2. **Make your changes** with focused commits
3. **Add tests** for new behaviour
4. **Run the full test suite** — `cargo test` must pass
5. **Run `cargo clippy`** — fix any warnings
6. **Open a PR** with a clear description of what and why

### Commit style

```
feat: add watch mode to nexa run
fix: resolver cycle detection on Windows paths
docs: update README quick start section
test: add coverage for missing entry file error
refactor: extract run_pipeline in compiler lib
```

---

## Code Style

- Follow standard Rust idioms (`cargo clippy` is the authority)
- Public types and functions should have doc comments
- Keep functions focused — if a function does IO, parsing, *and* side-effects, split it
- Prefer typed errors (`thiserror`) over `Box<dyn Error>` in new code
- Tests go in `#[cfg(test)]` modules at the bottom of the relevant file

---

## Release Process

Nexa uses three release channels, all automated via GitHub Actions:

| Channel | Trigger | Workflow |
|---------|---------|----------|
| **stable** | Push a `v*.*.*` tag | `.github/workflows/release.yml` |
| **snapshot** | Push to `main` (Rust files) | `.github/workflows/snapshot.yml` |
| **latest** | Alias for the most recent stable release | — |

### Publishing a stable release

```bash
# 1. Bump the version in all relevant Cargo.toml files
# 2. Commit and push the version bump
git commit -am "chore: bump version to v0.2.0"

# 3. Tag and push — this triggers the release workflow
git tag v0.2.0
git push origin v0.2.0
```

The workflow builds binaries for 5 targets, generates SHA-256 checksums, and publishes a GitHub Release with auto-generated release notes.

### Snapshot releases

Every push to `main` that touches Rust source automatically builds and updates the rolling `snapshot` pre-release at `https://github.com/nexa-lang-org/nexa-lang/releases/tag/snapshot`.

### Installer script

`setup.sh` at the repository root handles all three channels. Test it locally:

```bash
# Dry-run syntax check
sh -n setup.sh

# Test a local install (with a prebuilt binary already in place)
./setup.sh --channel stable --force
```

---

## Questions?

Open a [GitHub Discussion](https://github.com/nassime/Nexa-lang/discussions) or a [GitHub Issue](https://github.com/nassime/Nexa-lang/issues).
