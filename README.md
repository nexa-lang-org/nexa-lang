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
- **Project structure** — standardized layout enforced by the CLI

---

## Installation

### Prerequisites

- [Rust](https://rustup.rs/) 1.75 or later

### Build from source

```bash
git clone https://github.com/nassime/Nexa-lang.git
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
mkdir my-app && cd my-app
```

Create the required structure:

```
my-app/
  project.json
  nexa-compiler.yaml
  src/
    main/
      app.nx
```

**`project.json`**
```json
{
  "name": "my-app",
  "version": "0.1.0",
  "author": "Your Name",
  "main": "app.nx",
  "dependencies": []
}
```

**`nexa-compiler.yaml`**
```yaml
version: "0.1"
```

**`src/main/app.nx`**
```nx
package com.myapp;

app App {
  server { port: 3000; }

  public window HomePage {
    public render() => Component {
      return Page {
        Heading("Hello, Nexa!")
      };
    }
  }

  route "/" => HomePage;
}
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
nexa run     [<bundle.nexa>] [--project <dir>] [--port <port>] [--watch]
    Compile + start dev server, or serve an existing .nexa bundle directly

nexa build   [--project <dir>]
    Compile to src/dist/

nexa package [--project <dir>] [--output <file>]
    Package the project into a distributable .nexa bundle
```

`--project` defaults to the current directory.  
`--watch` enables hot-module reload via WebSocket.  
`--output` defaults to `<project-name>.nexa` in the current directory.

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

Nexa is a Rust workspace with three crates, each following Clean Architecture (domain / application / infrastructure / interfaces):

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
│   ├── application/     NexaProject config loading, build/run/package commands
│   └── interfaces/      Clap CLI definition (Cli, Commands)
│
└── server/
    ├── application/     AppState, SharedState, HMR broadcast
    └── interfaces/      Axum routes (/, /app.js, /ws WebSocket)
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

- [ ] `nexa init` scaffold command
- [x] Watch mode (`nexa run --watch`) with HMR via WebSocket
- [x] Error spans with rustc-style source locations
- [x] Clean Architecture (domain / application / infrastructure / interfaces)
- [x] `.nexa` bundle format (NXB bytecode + manifest + SHA-256 signature)
- [x] Optimizer pipeline (dead code removal, inlining, flattening, constant folding)
- [ ] Dependency resolution via `dependencies` in `project.json`
- [ ] Unit test runner (`nexa test`)
- [ ] Standard library (`std/`)
- [ ] Language Server Protocol (LSP) support

---

## License

MIT — see [LICENSE](LICENSE).
