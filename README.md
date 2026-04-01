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
nexa run    [--project <dir>] [--port <port>] [--watch]   Compile + start dev server
nexa build  [--project <dir>]                             Compile to src/dist/
```

`--project` defaults to the current directory.  
`--watch` enables hot-module reload via WebSocket.

---

## Architecture

Nexa is a Rust workspace with three crates, each following Clean Architecture (domain / application / infrastructure / interfaces):

```
crates/
├── compiler/
│   ├── domain/          AST nodes, Span value object
│   ├── application/
│   │   ├── ports/       SourceProvider trait (filesystem abstraction)
│   │   └── services/    Lexer, Parser, Resolver, SemanticAnalyzer, CodeGenerator
│   └── infrastructure/  FsSourceProvider (prod), MemSourceProvider (tests)
│
├── cli/
│   ├── application/     NexaProject config loading, build/run/watch commands
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
Lexer           tokenise source into a flat token stream
    │
    ▼
Parser          build a typed AST (Program, ClassDecl, Expr, …)
    │
    ▼
Resolver        load imported .nx files, merge declarations, detect cycles
    │
    ▼
SemanticAnalyzer check types, undefined references, route targets
    │
    ▼
CodeGenerator   emit index.html + app.js
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
- [ ] Dependency resolution via `dependencies` in `project.json`
- [ ] Unit test runner (`nexa test`)
- [ ] Standard library (`std/`)
- [ ] Language Server Protocol (LSP) support

---

## License

MIT — see [LICENSE](LICENSE).
