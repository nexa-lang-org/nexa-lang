<p align="center">
  <img src="assets/logo.png" alt="Nexa" width="120" />
</p>

# Nexa

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/built%20with-Rust-orange.svg)](https://www.rust-lang.org/)
[![Tests](https://img.shields.io/badge/tests-207%20passing-brightgreen.svg)](#)

**Nexa** is a statically-typed language that compiles to HTML + JavaScript (and WebAssembly). It lets you describe full-stack web applications — server config, data models, UI components, and routes — in a single, readable syntax with full type inference.

```nx
package com.myapp;

app App {
  server { port: 3000; }

  public window HomePage {
    async render() => Component {
      let items = await fetchData();
      return Page {
        Heading("Welcome to Nexa"),
        Column {
          Text(items)
        }
      };
    }
  }

  route "/" => HomePage;
}
```

> **Early-stage project.** The language and compiler are under active development. Syntax and APIs may change.

---

## Features

- **Hindley-Milner type inference** — Damas-Milner Algorithm W with let-polymorphism, generalization, and occurs-check
- **Typed language** — `String`, `Int`, `Bool`, `List<T>`, generics, interfaces, `async`/`await`
- **Generic classes & type erasure** — `Box<T>`, `List<T>` with a full JS runtime (`_NexaList`)
- **List literals** — `[1, 2, 3]` compiled to JS arrays; typed as `List<T>` by the HM engine
- **Async / await** — `async` methods + `await expr` compile to native JS async/await
- **Lazy loading** — `import("path.to.Module")` compiles to JS dynamic `import()`
- **Components & Windows** — declarative UI primitives that compile to DOM calls
- **Module system** — multi-module projects under `modules/<name>/`; cross-module imports
- **Package system** — `import models.User` resolves across module sources and `lib/`
- **Built-in routing** — `route "/" => HomePage` maps URLs to window classes
- **Integrated dev server** — `nexa run` compiles and serves with live reload
- **Watch mode** — `nexa run --watch` recompiles and reloads the browser on every save
- **`.nexa` bundle** — `nexa package` produces a distributable binary bundle (ZIP + AST + signature)
- **Package manager** — `nexa publish / install` + self-hosted registry with JWT auth and PostgreSQL
- **WASM backend** — `WasmCodegen` emits WebAssembly Text (WAT) from the IR; `wat2wasm` produces the binary
- **Build lockfile** — `nexa-build.lock` records SHA-256 per source file for reproducible builds
- **Security** — Ed25519 signatures, bcrypt passwords, rate limiting, validated JWT secret

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
| `snapshot` | `--channel snapshot` | Rolling build from `main` — unstable |

```sh
# Install the snapshot channel
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

---

## Quick Start

### 1. Create a project

```bash
nexa init my-app
cd my-app
```

Scaffolds the full module-based project structure:

```
my-app/
├── project.json          ← metadata + module list
├── nexa-compiler.yaml    ← compiler settings
├── .gitignore
└── modules/
    └── core/             ← default module
        ├── module.json
        └── src/
            ├── main/
            │   └── app.nx
            └── test/
```

```bash
nexa init my-app --author "Alice" --version 0.2.0
nexa init        # init in the current directory
```

### 2. Add a module

```bash
nexa module add api
# → modules/api/ created, project.json updated
```

### 3. Run the dev server

```bash
nexa run
# → Nexa dev server → http://localhost:3000
```

### 4. Build for production

```bash
nexa build
# → Build OK → modules/core/src/dist/
```

### 5. Package for distribution

```bash
nexa package
nexa package --module api   # package a specific module
```

---

## Language Reference

### Types

| Nexa      | JavaScript equivalent |
|-----------|-----------------------|
| `String`  | `string`              |
| `Int`     | `number`              |
| `Bool`    | `boolean`             |
| `List<T>` | `T[]` (`_NexaList`)   |
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

### Generic classes

```nx
public class Box<T> {
  T value;

  constructor(value: T) {
    this.value = value;
  }
}

// Type args are erased at JS runtime (type-safe at compile time):
let b = Box<Int>(42);
```

### Interfaces

```nx
public interface Repository<T> {
  findAll() => List<T>;
}
```

### Async / Await

```nx
public class ApiClient {
  async fetch(url: String) => String {
    let data = await loadUrl(url);
    return data;
  }
}
```

### List literals

```nx
let numbers = [1, 2, 3, 4, 5];
let names   = ["Alice", "Bob"];
```

### Lazy loading

Dynamic import inside an async method:

```nx
async load() => Void {
  let MathModule = await import("std.math.Math");
}
```

### Components

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

### Windows & Routing

```nx
public window AboutPage {
  public render() => Component {
    return Page {
      Heading("About"),
      Paragraph("Built with Nexa.")
    };
  }
}

route "/"      => HomePage;
route "/about" => AboutPage;
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

### Cross-module imports

```nx
// modules/api/src/main/Client.nx
import core.models.User;

public class Client {
  fetch() => User { ... }
}
```

---

## Project Structure

Every Nexa project follows this module-based layout:

```
my-app/
├── project.json           # { name, version, author, modules: ["core", "api"] }
├── nexa-compiler.yaml     # { version, main_module: "core" }
├── nexa-build.lock        # SHA-256 per source file (reproducible builds)
├── .gitignore
├── lib/                   # shared project dependencies
└── modules/
    ├── core/
    │   ├── module.json    # { name, main: "app.nx", dependencies: {} }
    │   ├── lib/           # module-local dependencies (gitignored)
    │   └── src/
    │       ├── main/      # .nx source files
    │       ├── test/      # unit tests
    │       └── dist/      # compiler output (gitignored)
    └── api/
        ├── module.json
        └── src/main/
```

---

## CLI Reference

```
nexa init    [<name>] [--author <name>] [--version <ver>] [--no-git]
    Scaffold a new Nexa project

nexa module add <name>
    Add a new module to the project

nexa run     [<bundle.nexa>] [--project <dir>] [--port <port>] [--watch]
    Compile + start dev server; or serve an existing .nexa bundle

nexa build   [--project <dir>]
    Compile all active modules to dist/

nexa package [--project <dir>] [--module <name>] [--output <file>]
    Package a module into a distributable .nexa bundle

nexa register [--registry <url>]
    Create a registry account (prompts for email + password)

nexa login   [--registry <url>]
    Log in and save credentials to ~/.nexa/credentials.json

nexa publish [--project <dir>] [--module <name>] [--registry <url>]
    Build and publish a module to the registry

nexa install [<package[@version]>] [--project <dir>] [--module <name>]
    Install a package (--module installs into that module's lib/)

nexa token   create|list|revoke
    Manage long-lived API tokens

nexa doctor  [--project <dir>]
    Diagnose project configuration issues
```

`--project` defaults to the current directory.  
`--registry` defaults to `~/.nexa/credentials.json`, then `https://registry.nexa-lang.org`.

---

## Package Manager

### Declaring dependencies

`project.json` (project-wide):

```json
{
  "name": "my-app",
  "version": "0.1.0",
  "modules": ["core", "api"],
  "dependencies": { "ui-kit": "^1.0.0" }
}
```

`modules/api/module.json` (module-specific):

```json
{
  "name": "api",
  "main": "app.nx",
  "dependencies": { "http-client": "1.2.0" }
}
```

### Workflow

```bash
nexa register && nexa login
nexa install ui-kit               # → lib/ui-kit@1.0.0/
nexa install http-client --module api  # → modules/api/lib/http-client@1.2.0/
nexa publish --module core
```

### Registry API

| Method | Path | Description |
|--------|------|-------------|
| `POST` | `/v1/auth/register` | Create account |
| `POST` | `/v1/auth/login` | Log in → JWT |
| `POST` | `/v1/packages/:name/publish` | Publish bundle (Bearer) |
| `GET`  | `/v1/packages/:name` | Package info |
| `GET`  | `/v1/packages/:name/:version/download` | Download bundle |
| `GET`  | `/v1/packages?q=` | Search |

---

## `.nexa` Bundle Format

```
my-app.nexa  (ZIP)
├── app.nxb        ← optimized AST (bincode, magic NXB\x01)
├── manifest.json  ← { name, version, nexa_version, nxb_version, created_at }
└── signature.sig  ← Ed25519 signature (publisher signs, registry verifies)
```

Four optimizer passes run before serialization:

| Pass | What it does |
|------|-------------|
| **Dead code removal** | Strips declarations unreachable from any route |
| **Component inlining** | Inlines trivial single-render components |
| **Tree flattening** | Collapses `Page { Page { … } }` → `Page { … }` |
| **Constant folding** | `2 + 3` → `5`, `"a" + "b"` → `"ab"` |

---

## Architecture

Nexa is a Rust workspace with four crates following Clean Architecture:

```
crates/
├── compiler/
│   ├── domain/         AST, IR (IrModule / IrExpr / IrStmt / IrType), Span
│   ├── application/
│   │   ├── ports/      SourceProvider trait
│   │   └── services/   Lexer, Parser, Resolver (5-step), SemanticAnalyzer
│   │                   (7 passes incl. HM), Optimizer, Packager,
│   │                   CodeGenerator (JS), WasmCodegen (WAT)
│   └── infrastructure/ FsSourceProvider, MemSourceProvider
│
├── cli/
│   ├── application/    project.rs, commands/ (init, build, module, registry,
│   │                   token, config, doctor), credentials, updater
│   └── interfaces/     Clap CLI
│
├── server/
│   └── interfaces/     Axum dev server (/, /app.js, /ws HMR)
│
└── registry/
    ├── domain/         User, Package entities
    ├── application/    AuthService (JWT/bcrypt), PackagesService
    ├── infrastructure/ PgUserStore, PgPackageStore (sqlx)
    └── interfaces/     Axum HTTP API (/v1/auth/*, /v1/packages/*)
```

**Compilation pipeline:**

```
.nx source
    │
    ▼
Lexer              tokenise → flat token stream
    │
    ▼
Parser             token stream → typed AST
    │
    ▼
Resolver           5-step import resolution (relative → module → lib module
    │              → lib project → cross-module); cycle detection
    ▼
SemanticAnalyzer   7 passes:
    │              1. Name collection
    │              2. Reference validation (extends / implements)
    │              3. Import validation
    │              4. Route validation
    │              5. Type checking (annotations + return types)
    │              6. Generic param validation
    │              7. Hindley-Milner type inference (Damas-Milner Algorithm W,
    │                 let-polymorphism, generalize/instantiate, occurs-check)
    │
    ├── nexa build / nexa run ──► Lower (AST → IR) ──► CodeGenerator (JS)
    │
    ├── nexa wasm ──────────────► Lower (AST → IR) ──► WasmCodegen (WAT)
    │
    └── nexa package
            │
            ▼
        Optimizer  (4 passes)
            │
            ▼
        Packager   (NXB + manifest + Ed25519 signature)
```

---

## Contributing

Contributions are welcome! Please read [CONTRIBUTING.md](CONTRIBUTING.md) first.

---

## Roadmap

- [x] `nexa init` scaffold command + module system (`nexa module add`)
- [x] Watch mode with HMR via WebSocket
- [x] Error spans with rustc-style source locations
- [x] Clean Architecture across all four crates
- [x] `.nexa` bundle format (NXB + manifest + Ed25519 signature)
- [x] Optimizer pipeline (4 passes)
- [x] Package manager + self-hosted registry (JWT, bcrypt, rate limiting)
- [x] Multi-module build (`active_modules`, cross-module imports)
- [x] IR (target-agnostic intermediate representation)
- [x] WASM backend (WAT from IR)
- [x] Hindley-Milner type inference — Damas-Milner Algorithm W with let-polymorphism
- [x] Generic classes — syntax + Pass 6 validation + JS type erasure + `_NexaList` runtime
- [x] `async`/`await` — async methods + JS codegen
- [x] List literals `[...]` + lazy `import("path")` dynamic import
- [x] Build lockfile (SHA-256 per source file)
- [x] Coverage CI (cargo-tarpaulin)
- [ ] Standard library runtime bodies (`std.io`, `std.math`, `std.str`, `std.collections`)
- [ ] Semver constraint resolution for `dependencies`
- [ ] Unit test runner (`nexa test`)
- [ ] Language Server Protocol (LSP) support
- [ ] Garbage collector (for non-JS runtimes)
- [ ] Thread / coroutines (Web Workers + WASM threads)

---

## License

MIT — see [LICENSE](LICENSE).
