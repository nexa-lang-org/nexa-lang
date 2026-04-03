<p align="center">
  <img src="icons/nexa.png" width="96" alt="Nexa Logo" />
</p>

<h1 align="center">Nexa Language Support</h1>

<p align="center">
  Official VS Code extension for the <strong>Nexa programming language</strong> —
  syntax highlighting, smart snippets, and language configuration.
</p>

<p align="center">
  <a href="https://github.com/nexa-lang-org/nexa-lang"><img src="https://img.shields.io/badge/github-na2sime%2FNexa--lang-blue?logo=github" alt="GitHub" /></a>
  <a href="https://registry.nexa-lang.org"><img src="https://img.shields.io/badge/registry-registry.nexa--lang.org-green" alt="Registry" /></a>
  <img src="https://img.shields.io/badge/VS%20Code-%5E1.80.0-blue?logo=visualstudiocode" alt="VS Code" />
  <img src="https://img.shields.io/badge/language-.nexa%20%7C%20.nx-orange" alt="File types" />
</p>

---

## Features

### Syntax Highlighting

Full grammar coverage for every Nexa construct:

- **Keywords** — `app`, `class`, `interface`, `component`, `window`, `route`, `import`, `package`, `let`, `if`, `else`, `for`, `while`, `return`, …
- **Built-in types** — `Int`, `String`, `Bool`, `Void`, `List<T>`
- **Visibility modifiers** — `public`, `private`, `extends`, `implements`
- **Literals** — strings with escape sequences, integers, `true` / `false`
- **Operators** — arithmetic, comparison, logical, arrow `=>`
- **Comments** — line comments `//`
- **Type annotations** — `: Type` syntax in parameters and variable declarations
- **Named entities** — class names, method names, function calls highlighted distinctly

### Snippets (18 built-in)

| Prefix | Expands to |
|--------|-----------|
| `app` | Full app scaffold with server, window, and route |
| `class` | Class declaration with constructor |
| `classe` | Class with `extends` |
| `interface` | Interface declaration |
| `comp` | UI component with `render()` |
| `window` | Page/window with `render()` |
| `method` | Public method |
| `pmethod` | Private method |
| `ctor` | Constructor |
| `let` | Variable declaration |
| `lett` | Typed variable declaration |
| `if` | If statement |
| `ife` | If / Else statement |
| `for` | For-in loop |
| `while` | While loop |
| `route` | Route declaration |
| `server` | Server configuration block |
| `import` | Import declaration |

### Language Configuration

- Auto-close pairs: `{}`, `()`, `""`, `[]`
- Smart indentation on `{` / `}`
- Correct word boundaries for Nexa identifiers

---

## Getting Started

### Install Nexa

```sh
curl --proto '=https' --tlsv1.2 -sSf \
  https://raw.githubusercontent.com/nexa-lang-org/nexa-lang/main/setup.sh | sh
```

### Create a project

```sh
nexa init my-app
cd my-app
nexa run
```

---

## Nexa Language at a Glance

```nexa
package com.myapp;

import models.User;

app App {

  server {
    port: 3000;
  }

  public class User {
    String name;
    Int    age;

    constructor(name: String, age: Int) {
      this.name = name;
      this.age  = age;
    }

    public isAdult() => Bool {
      return this.age >= 18;
    }
  }

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

  public window HomePage {
    public render() => Component {
      let title = "Hello, Nexa!";

      return Page {
        Heading(title)
      };
    }
  }

  route "/" => HomePage;
}
```

---

## File Types

| Extension | Description |
|-----------|-------------|
| `.nexa` | Nexa source file (recommended) |
| `.nx` | Nexa source file (short form) |

---

## Requirements

- **VS Code** 1.80.0 or later
- **Nexa CLI** — install via the one-liner above

---

## Extension Settings

This extension has no configurable settings — it activates automatically for `.nexa` and `.nx` files.

---

## Feedback & Contributing

Found a highlighting bug or want a new snippet?

- Open an issue: [github.com/nexa-lang-org/nexa-lang/issues](https://github.com/nexa-lang-org/nexa-lang/issues)
- The grammar lives in [`editors/vscode/syntaxes/nexa.tmLanguage.json`](syntaxes/nexa.tmLanguage.json)
- Snippets live in [`editors/vscode/snippets/nexa.json`](snippets/nexa.json)

---

## Release Notes

### 0.1.0

Initial release — syntax highlighting, 18 snippets, language configuration.
