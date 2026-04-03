<p align="center">
  <img src="assets/logo.png" alt="Nexa" width="120" />
</p>

# Nexa

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/built%20with-Rust-orange.svg)](https://www.rust-lang.org/)

**Nexa** is a statically-typed language that compiles to HTML + JavaScript. It lets you describe full-stack web applications — server config, data models, UI components, and routes — in a single, readable syntax.

```nx
package com.myapp;

app App {
  server { port: 3000; }

  public window HomePage {
    public render() => Component {
      return Page {
        Heading("Welcome to Nexa")
      };
    }
  }

  route "/" => HomePage;
}
```

> **Early-stage project.** The language and compiler are under active development. Syntax and APIs may change.

---

## Features

- **Typed language** — `String`, `Int`, `Bool`, `List<T>`, generics, interfaces
- **Components & Windows** — declarative UI primitives that compile to DOM calls
- **Package system** — `import models.User` resolves across `src/main/` and `src/libs/`
- **Built-in routing** — `route "/" => HomePage` maps URLs to window classes
- **Integrated dev server** — `nexa run` compiles and serves with live reload
- **Watch mode** — `nexa run --watch` recompiles and reloads the browser on every save
- **`.nexa` bundle** — `nexa package` produces a distributable binary bundle (ZIP + AST + signature)
- **Package manager** — `nexa publish / install` + a self-hosted registry with JWT auth and PostgreSQL
- **Project structure** — standardized layout enforced by the CLI

---

## Installation

### One-line install (macOS / Linux)

```sh
curl --proto '=https' --tlsv1.2 -sSf \
  https://raw.githubusercontent.com/nexa-lang-org/nexa-lang/main/setup.sh | sh
```

The installer:
- Downloads a prebuilt binary for your platform (x86_64 or aarch64)
- Verifies the SHA-256 checksum before installing
- Falls back to building from source if no binary is available (installs Rust via `rustup` automatically)
- Adds `~/.nexa/bin` to your `PATH`

**Windows:** download `nexa-windows-x86_64.zip` from [Releases](https://github.com/nexa-lang-org/nexa-lang/releases/latest).

### Release channels

| Channel | Command | Description |
|---------|---------|-------------|
| `stable` | *(default)* | Latest tagged release — recommended |
| `latest` | `--channel latest` | Alias for `stable` |
| `snapshot` | `--channel snapshot` | Rolling build from `main` — unstable |

```sh
# Install the snapshot (dev) channel
curl ... | sh -s -- --channel snapshot

# Pin a specific version
curl ... | sh -s -- --version v0.2.0
```

### Build from source

Requires [Rust](https://rustup.rs/) 1.75+:

```bash
git clone https://github.com/nexa-lang-org/nexa-lang.git
cd Nexa-lang
cargo install --path crates/cli
```

Verify:

```bash
nexa --version
```

---

## Quick Start

### 1. Create a project

```bash
nexa init my-app
cd my-app
```

This scaffolds the full project structure and initialises a git repository:

```
my-app/
├── project.json        ← metadata, dependencies
├── nexa-compiler.yaml  ← compiler settings, registry config
├── .gitignore
└── src/
    └── main/
        └── app.nx      ← entry point
```

Pass `--author` to set the author name, or let the CLI read it from `git config`:

```bash
nexa init my-app --author "Alice" --version 0.2.0
nexa init        # init in the current directory
```

### 2. Run the dev server

```bash
nexa run
# → Nexa dev server → http://localhost:3000
```

### 3. Watch mode (HMR)

```bash
nexa run --watch
# Recompiles and reloads the browser on every .nx save
```

### 4. Build for production

```bash
nexa build
# → Build OK → src/dist/
```

Output: `src/dist/index.html` + `src/dist/app.js`.

### 5. Package for distribution

```bash
nexa package
# → Package OK → my-app.nexa
```

Produces a single distributable file. See [`.nexa` bundle format](#nexa-bundle-format) below.

---

## Language Reference

### Types

| Nexa      | JavaScript equivalent |
|-----------|-----------------------|
| `String`  | `string`              |
| `Int`     | `number`              |
| `Bool`    | `boolean`             |
| `List<T>` | `T[]`                 |
| `Void`    | `void`                |

### Classes

```nx
public class User {
  String name;
  Int age;

  constructor(name: String, age: Int) {
    this.name = name;
    this.age = age;
  }

  public isAdult() => Bool {
    return this.age >= 18;
  }
}
```

### Interfaces

```nx
public interface Repository<T> {
  findAll() => List<T>;
}
```

### Components

Reusable UI elements that render DOM nodes:

```nx
public component UserCard {
  private String name;

  constructor(name: String) {
    this.name = name;
  }

  public render() => Component {
    return Column {
      Heading(this.name)
    };
  }
}
```

### Windows

Full-page views mapped to routes:

```nx
public window AboutPage {
  public render() => Component {
    return Page {
      Heading("About"),
      Paragraph("Built with Nexa.")
    };
  }
}
```

### UI Primitives

| Primitive                      | Description               |
|--------------------------------|---------------------------|
| `Page { ... }`                 | Root page container       |
| `Column { ... }`               | Vertical flex container   |
| `Row { ... }`                  | Horizontal flex container |
| `Heading(text)`                | `<h1>` heading            |
| `Paragraph(text)`              | `<p>` paragraph           |
| `Text(content)`                | Inline text node          |
| `Button(label, onClick)`       | Clickable button          |
| `Input(placeholder, onChange)` | Text input field          |

### Routing

```nx
route "/"      => HomePage;
route "/about" => AboutPage;
```

### Imports

```nx
import models.User;               // → src/main/models/User.nx
import libs.validation.Validator; // → src/libs/validation/Validator.nx
```

### Control flow

```nx
if (count > 0) {
  return Page { Heading("Items found") };
} else {
  return Page { Text("Empty") };
}
```

---

## Project Structure

Every Nexa project follows this layout:

```
my-app/
├── project.json          # Project metadata (name, version, author, main)
├── nexa-compiler.yaml    # Compiler configuration
└── src/
    ├── main/             # Source .nx files (required)
    │   └── app.nx        # Entry point declared in project.json
    ├── libs/             # Reusable libraries (auto-created)
    ├── test/             # Future: unit tests (auto-created)
    ├── .nexa/            # Compiler internals / cache (auto-created, gitignored)
    └── dist/             # Compiler output (auto-created, gitignored)
        ├── index.html
        └── app.js
```

---

## CLI Reference

```
nexa init    [<name>] [--author <name>] [--version <ver>] [--no-git]
    Scaffold a new Nexa project (creates directory, git repo, hello-world app)

nexa run     [<bundle.nexa>] [--project <dir>] [--port <port>] [--watch]
    Compile + start dev server, or serve an existing .nexa bundle directly

nexa build   [--project <dir>]
    Compile to src/dist/

nexa package [--project <dir>] [--output <file>]
    Package the project into a distributable .nexa bundle

nexa register [--registry <url>]
    Create an account on a registry (prompts for email + password)

nexa login   [--registry <url>]
    Log in to a registry and save credentials to ~/.nexa/credentials.json

nexa publish [--project <dir>] [--registry <url>]
    Build a .nexa bundle and publish it to the registry

nexa install [<package[@version]>] [--project <dir>]
    Install a package from the registry into nexa-libs/
    Omit <package> to install all dependencies from project.json
```

`--project` defaults to the current directory.  
`--registry` defaults to the URL stored in `~/.nexa/credentials.json`, then to `https://registry.nexa-lang.org`.  
`--watch` enables hot-module reload via WebSocket.  
`--output` defaults to `<project-name>.nexa` in the current directory.

---

## Package Manager

Nexa comes with a built-in package manager backed by a self-hosted registry.

### Declaring dependencies

In `project.json`, `dependencies` is a map of package name → semver constraint:

```json
{
  "name": "my-app",
  "version": "0.1.0",
  "dependencies": {
    "ui-kit": "^1.0.0",
    "auth-utils": "2.3.1"
  }
}
```

### Workflow

```bash
# 1. Create an account
nexa register

# 2. Log in (saves JWT to ~/.nexa/credentials.json)
nexa login

# 3. Publish your library
nexa publish --project path/to/my-lib

# 4. Install a dependency
nexa install ui-kit
nexa install ui-kit@1.2.0   # pin a specific version

# 5. Install all deps from project.json
nexa install
```

Packages are extracted to `nexa-libs/<name>@<version>/` (gitignored). A JSON lockfile is written to `nexa-libs/.lock`.

### Private registries

Add private registries in `nexa-compiler.yaml`. They are tried before the public registry:

```yaml
version: "0.1"
registry: "https://registry.nexa-lang.org"
private_registries:
  - url: "https://corp.registry.example.com"
    key: "sk_live_abc123"
```

### Registry API

| Method | Path | Description |
|--------|------|-------------|
| `POST` | `/auth/register` | Create an account → `{ token }` |
| `POST` | `/auth/login` | Log in → `{ token }` |
| `POST` | `/packages/:name/publish` | Publish a `.nexa` bundle (Bearer auth) |
| `GET`  | `/packages/:name` | Package info + version list |
| `GET`  | `/packages/:name/:version/download` | Download the `.nexa` bundle |
| `GET`  | `/packages?q=&page=&per_page=` | Search packages |

### Running a registry

The registry is a separate binary (`nexa-registry`) backed by PostgreSQL:

```bash
# Start the DB
docker compose up -d registry-db

# Start the registry
DATABASE_URL=postgres://nexa:nexa@localhost:5432/nexa_registry \
JWT_SECRET=change-me-in-prod \
cargo run -p nexa-registry
# → listening on port 4000
```

See `docker-compose.yml` at the workspace root for the full Docker Compose setup.

---

## `.nexa` Bundle Format

`nexa package` produces a distributable binary bundle analogous to an Android APK or an Electron ASAR — a ZIP archive that can be deployed and run directly without recompiling from source.

```
my-app.nexa  (ZIP)
├── app.nxb        ← optimized AST in binary format
├── manifest.json  ← auto-generated metadata
└── signature.sig  ← SHA-256 integrity hash
```

### `app.nxb` — compiled bytecode

The `.nxb` file contains the fully-resolved, semantically-validated, and **optimized** AST serialized in binary format (4-byte magic `NXB\x01` + [bincode](https://github.com/bincode-org/bincode) payload).

Before serialization, the compiler runs four optimization passes over the AST:

| Pass | What it does |
|------|-------------|
| **Dead code removal** | Strips declarations unreachable from any route or live reference |
| **Component inlining** | Inlines trivial zero-field, single-`render` components at their call sites |
| **Tree flattening** | Collapses redundant nested blocks of the same type (`Page { Page { … } }` → `Page { … }`) |
| **Constant folding** | Evaluates constant expressions at compile time (`2 + 3` → `5`, `"a" + "b"` → `"ab"`) |

When `nexa run my-app.nexa` is called, the bundle is extracted, the signature is validated, the AST is deserialized, and the `CodeGenerator` produces HTML + JS on the fly — no `.nx` sources needed.

### `manifest.json` — bundle metadata

Auto-generated at package time:

```json
{
  "name": "my-app",
  "version": "0.1.0",
  "nexa_version": "0.1.0",
  "nxb_version": 1,
  "created_at": 1743600000
}
```

### `signature.sig` — integrity check

A hex-encoded SHA-256 hash of the concatenated `app.nxb` and `manifest.json` bytes. Verified automatically by `nexa run` before the bundle is loaded.

---

## Architecture

Nexa is a Rust workspace with four crates, each following Clean Architecture (domain / application / infrastructure / interfaces):

```
crates/
├── compiler/
│   ├── domain/          AST nodes, Span value object
│   ├── application/
│   │   ├── ports/       SourceProvider trait (filesystem abstraction)
│   │   └── services/    Lexer, Parser, Resolver, SemanticAnalyzer,
│   │                    Optimizer, Packager, CodeGenerator
│   └── infrastructure/  FsSourceProvider (prod), MemSourceProvider (tests)
│
├── cli/
│   ├── application/     project.json / nexa-compiler.yaml config, credentials,
│   │                    run/build/package/login/register/publish/install commands
│   └── interfaces/      Clap CLI definition (Cli, Commands)
│
├── server/
│   ├── application/     AppState, SharedState, HMR broadcast
│   └── interfaces/      Axum routes (/, /app.js, /ws WebSocket)
│
└── registry/
    ├── domain/          User, Package, PackageVersion entities
    ├── application/
    │   ├── ports/       UserStore + PackageStore traits
    │   └── services/    AuthService (JWT/bcrypt), PackagesService
    ├── infrastructure/  PgUserStore, PgPackageStore (sqlx)
    └── interfaces/      Axum HTTP API (auth, publish, download, search)
```

**Compilation pipeline:**

```
.nx source
    │
    ▼
Lexer            tokenise source into a flat token stream
    │
    ▼
Parser           build a typed AST (Program, ClassDecl, Expr, …)
    │
    ▼
Resolver         load imported .nx files, merge declarations, detect cycles
    │
    ▼
SemanticAnalyzer check types, undefined references, route targets
    │
    ├── nexa build / nexa run
    │       ▼
    │   CodeGenerator   emit index.html + app.js
    │
    └── nexa package
            ▼
        Optimizer        dead code removal → inlining → flattening → constant folding
            │
            ▼
        Packager         serialize AST to app.nxb, generate manifest, sign
```

---

## Contributing

Contributions are welcome! Please read [CONTRIBUTING.md](CONTRIBUTING.md) first.

---

## Roadmap

- [x] `nexa init` scaffold command
- [x] Watch mode (`nexa run --watch`) with HMR via WebSocket
- [x] Error spans with rustc-style source locations
- [x] Clean Architecture (domain / application / infrastructure / interfaces)
- [x] `.nexa` bundle format (NXB bytecode + manifest + SHA-256 signature)
- [x] Optimizer pipeline (dead code removal, inlining, flattening, constant folding)
- [x] Package manager (`nexa publish / install`) + self-hosted registry
- [x] JWT auth + bcrypt password hashing for the registry
- [x] Private registry support in `nexa-compiler.yaml`
- [ ] Semver constraint resolution for `dependencies` in `project.json`
- [ ] Unit test runner (`nexa test`)
- [ ] Standard library (`std/`)
- [ ] Language Server Protocol (LSP) support

---

## License

MIT — see [LICENSE](LICENSE).
