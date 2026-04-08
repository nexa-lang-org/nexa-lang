# Nexa — Plan multi-cibles & multi-types d'applications

> **Fichier de travail — ne pas committer / pusher.**
> Date : 2026-04-08

---

## 1. Vision

Nexa passe d'un compilateur web-only à un **langage universel** capable de produire :

| Type d'app    | Exemples concrets                                  | Cible de compilation                                                                                       |
|---------------|----------------------------------------------------|------------------------------------------------------------------------------------------------------------|
| **web**       | SPA, dashboard, landing page                       | **Bundle `.nexa`** (ZIP + AST + signature Ed25519 — distribuable via registry si souhaité, mais optionnel) |
| **backend**   | REST API, serveur WebSocket, microservice          | **Binaire Rust natif**                                                                                     |
| **cli**       | Outil en ligne de commande                         | **Binaire Rust natif**                                                                                     |
| **desktop**   | App macOS / Windows / Linux avec UI                | **Rust shell + CEF (Chromium) + AST**                                                                      |
| **package**   | Bibliothèque réutilisable par d'autres projets     | **Bundle `.nexa`** (ZIP + AST + signature Ed25519, distribué via registry)                                 |
| **mobile**    | iOS / Android *(phase future)*                     | À définir                                                                                                  |

> **Décisions clés :**
> - `backend` et `cli` → **binaire Rust natif** (pas Node.js)
> - `desktop` → **CEF (Chromium)** embarqué — rendu 100% identique cross-platform
> - `type` et `platforms` vivent dans **`module.json`** — pas dans le source `.nx`
> - Un projet peut avoir des modules de types différents (ex: `web` + `backend` dans le même projet)

---

## 2. Intégration avec le système de config existant

### 2.1 Fichiers de config existants (structure inchangée)

```
project.json          ← global : name, version, author, modules[], dependencies
nexa-compiler.yaml    ← compiler : main_module, include/exclude, registries
modules/<name>/
  module.json         ← par module : name, main, dependencies  ← on étend ici
  src/main/app.nx     ← source Nexa — INCHANGÉ
```

Le build lit les configs **avant** de parser le source `.nx`.
Logique : `module.json` → détermine comment compiler → compile le `.nx`.

Mettre `type`/`platforms` dans le source `.nx` créerait une dépendance circulaire :
le build system devrait parser le source pour savoir comment le compiler.

### 2.2 Extension de `module.json` — nouveaux champs : `type`, `platforms`, `desktop`

```json
// modules/api/module.json
{
  "name": "api",
  "main": "app.nx",
  "type": "backend",
  "platforms": ["native-linux", "native-macos"],
  "dependencies": {}
}
```

```json
// modules/web/module.json
{
  "name": "web",
  "main": "app.nx",
  "type": "web",
  "platforms": ["browser"],
  "dependencies": {}
}
```

```json
// modules/desktop-app/module.json
{
  "name": "desktop-app",
  "main": "app.nx",
  "type": "desktop",
  "platforms": ["macos", "windows", "linux"],
  "desktop": {
    "title": "NoteApp",
    "width": 1200,
    "height": 800,
    "resizable": true,
    "icon": "assets/icon.png"
  },
  "dependencies": {}
}
```

```json
// modules/my-lib/module.json
{
  "name": "my-lib",
  "main": "lib.nx",
  "type": "package",
  "version": "1.2.0"
}
```

> **Rétro-compatibilité** : `type` absent → `web` par défaut. Tous les projets existants
> continuent de fonctionner sans aucune modification.

### 2.3 Platforms disponibles

```
browser          web (bundle .nexa)
macos            desktop macOS   → CEF + .app
windows          desktop Windows → CEF + .exe
linux            desktop Linux   → CEF + AppImage / .deb
native           backend/cli → OS courant au moment du build
native-macos     backend/cli cross-compilé pour macOS
native-windows   backend/cli cross-compilé pour Windows
native-linux     backend/cli cross-compilé pour Linux
ios              mobile (futur)
android          mobile (futur)
```

### 2.4 Valeurs par défaut si `platforms` est absent

| Type      | Default platforms       |
|-----------|-------------------------|
| `web`     | `["browser"]`           |
| `backend` | `["native"]`            |
| `cli`     | `["native"]`            |
| `desktop` | `["macos"]`             |
| `package` | aucune (agnostique)     |

### 2.5 Projet fullstack — exemple complet

```
my-saas/
  project.json              { "modules": ["web", "api"] }
  nexa-compiler.yaml        main_module: "web"
  modules/
    web/
      module.json           { "type": "web", "platforms": ["browser"] }
      src/main/app.nx       ← app block inchangé
    api/
      module.json           { "type": "backend", "platforms": ["native-linux"] }
      src/main/app.nx       ← app block inchangé
```

`nexa build` → compile les deux modules selon leur `module.json` :

```
dist/
  web/browser/          ← bundle .nexa (HTML + JS)
  api/native-linux/     ← binaire Rust
```

---

## 3. Source `.nx` — aucun changement

L'app block **reste exactement comme aujourd'hui**. Aucune nouvelle syntaxe dans le source Nexa.

```nexa
// modules/web/src/main/app.nx — inchangé
app TodoApp {
    server { port: 3000; }
    public window Home { ... }
    route "/" => Home;
}
```

```nexa
// modules/api/src/main/app.nx — inchangé
app ApiServer {
    main() => Void {
        let server = HttpServer(8080);
        server.onRequest((method, path, body) => {
            return "{ \"ok\": true }";
        });
        await server.start();
    }
}
```

```nexa
// modules/desktop-app/src/main/app.nx — inchangé
app NoteApp {
    public window Home { ... }
    route "/" => Home;
}
```

```nexa
// modules/my-lib/src/lib.nx
package mylib;
public class Calculator {
    add(a: Int, b: Int) => Int { return a + b; }
    multiply(a: Int, b: Int) => Int { return a * b; }
}
```

---

## 4. CLI — commandes

```
# Création de projet (génère module.json avec type + platforms pré-remplis)
nexa new my-api   --type backend
nexa new my-tool  --type cli
nexa new my-app   --type desktop
nexa new my-lib   --type package

# Ajout d'un module typé dans un projet existant
nexa module add api   --type backend   --platforms native-linux,native-macos
nexa module add front --type web

# Build — lit module.json de chaque module actif, compile chaque (module × platform)
nexa build
nexa build --release

# Surcharge ponctuelle (pour CI ou debug)
nexa build --module api --platform native-linux

# Dev (toujours sur la platform courante)
nexa run
nexa run --module api

# Package & registry (existants, inchangés)
nexa package
nexa publish
nexa install mylib
```

---

## 5. Architecture du compilateur

### 5.1 Pipeline actuel

```
project.json + nexa-compiler.yaml + module.json
  → NexaProject::load()
  → pour chaque module actif :
       compile_project_file(entry, src_root) → HTML + JS
       → dist/<module>/
```

### 5.2 Pipeline cible

```
project.json + nexa-compiler.yaml + module.json (+ type + platforms + desktop)
  → NexaProject::load()   ← lit type/platforms/desktop depuis module.json
  → pour chaque module actif :
       Source .nx → Lexer → Parser → AST → Lowerer → IR
       → TargetDispatcher  (reçoit module.type + module.effective_platforms())
            → pour chaque platform (en parallèle via Rayon) :
                 WebTarget      → HTML+JS → bundle .nexa    (web,     browser)
                 RustTarget     → main.rs → cargo build     (backend/cli, native-*)
                 DesktopTarget  → HTML+JS + shell CEF       (desktop, macos/windows/linux)
                 PackageTarget  → bundle .nexa (AST signé)  (package)
       → dist/<module>/<platform>/
```

### 5.3 Extension de `ModuleConfig` dans `project.rs`

```rust
// crates/cli/src/application/project.rs

#[derive(Debug, Clone, Deserialize, PartialEq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum AppType { #[default] Web, Backend, Cli, Desktop, Package }

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum Platform {
    Browser,
    MacOs, Windows, Linux,
    Native, NativeMacOs, NativeWindows, NativeLinux,
    Ios, Android,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct DesktopConfig {
    pub title: String,
    pub width: u32,
    pub height: u32,
    #[serde(default = "default_true")]
    pub resizable: bool,
    pub icon: Option<String>,
}

// ModuleConfig existant — on ajoute les nouveaux champs
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct ModuleConfig {
    pub name: String,
    pub main: String,
    #[serde(default)]
    pub dependencies: HashMap<String, String>,

    // ── Nouveaux champs ──────────────────────────────────────────────
    #[serde(default)]
    pub r#type: AppType,
    #[serde(default)]
    pub platforms: Vec<Platform>,
    pub desktop: Option<DesktopConfig>,
    pub version: Option<String>,   // pour type: package
}

impl ModuleConfig {
    /// Retourne les platforms effectives (defaults si platforms: [] dans module.json).
    pub fn effective_platforms(&self) -> Vec<Platform> {
        if !self.platforms.is_empty() {
            return self.platforms.clone();
        }
        match self.r#type {
            AppType::Web                    => vec![Platform::Browser],
            AppType::Backend | AppType::Cli => vec![Platform::Native],
            AppType::Desktop                => vec![Platform::MacOs],
            AppType::Package                => vec![],
        }
    }
}
```

### 5.4 Dispatcher — `build.rs` étendu

```rust
// build_module est appelé dans la boucle existante de build.rs
fn build_module(proj: &NexaProject, mod_name: &str) {
    let module_cfg = &proj.modules[mod_name];
    let platforms = module_cfg.effective_platforms();

    // Compile le source une seule fois → IR
    let ir = compile_to_ir(&proj.module_entry(mod_name), &proj.module_src_root(mod_name));

    // Lance un build par platform, en parallèle
    platforms.par_iter().for_each(|platform| {
        let out_dir = proj.root()
            .join("dist").join(mod_name).join(platform.as_str());

        match (&module_cfg.r#type, platform) {
            (AppType::Web, Platform::Browser)      => web::build(&ir, &out_dir),
            (AppType::Backend | AppType::Cli, p)   => rust::build(&ir, p, &out_dir),
            (AppType::Desktop, p)                  => desktop::build(&ir, p, module_cfg, &out_dir),
            (AppType::Package, _)                  => package::build(&ir, &out_dir),
            _ => eprintln!("combinaison type/platform non supportée"),
        }
    });
}
```

---

## 6. Codegen Rust — Backend & CLI

### 6.1 Mapping de types Nexa → Rust

| Nexa        | Rust               |
|-------------|--------------------|
| `Int`       | `i64`              |
| `String`    | `String`           |
| `Bool`      | `bool`             |
| `Void`      | `()`               |
| `List<T>`   | `Vec<T>`           |
| `(A) => B`  | `impl Fn(A) -> B`  |
| classe Nexa | `struct` + `impl`  |
| `async fn`  | `async fn` + tokio |

### 6.2 Crates stdlib Rust

| Stdlib Nexa             | Crate Rust          | Rôle                      |
|-------------------------|---------------------|---------------------------|
| `std.server.HttpServer` | `axum`              | Serveur HTTP async        |
| `std.net.HttpClient`    | `reqwest`           | Client HTTP               |
| `std.net.Socket`        | `tokio-tungstenite` | WebSocket                 |
| `std.io.File`           | `tokio::fs`         | I/O fichier async         |
| `std.io.Console`        | `println!`          | Output terminal           |
| `std.process.Process`   | `std::process`      | Exit, PID…                |
| `std.process.Env`       | `std::env`          | Variables d'environnement |
| `std.async.*`           | `tokio::sync`       | Future, channel           |
| `std.collections.*`     | `std::collections`  | HashMap, Vec…             |

### 6.3 Sortie générée

```
_nexa_out/<module>/<platform>/
  src/main.rs       ← code Nexa transpilé en Rust + impls stdlib natives
  Cargo.toml        ← dépendances selon les modules stdlib utilisés

dist/<module>/<platform>/
  <name>            ← binaire final (après cargo build --release)
```

### 6.4 Nouveaux modules codegen

```
crates/compiler/src/application/services/
  codegen.rs          ← existant (web → HTML+JS)
  codegen_rust.rs     ← nouveau  (backend/cli → Rust source + cargo build)
  codegen_desktop.rs  ← nouveau  (desktop → HTML+JS + shell CEF)
```

---

## 7. Stratégie Desktop — Shell Rust + CEF

**Décision** : CEF pour un rendu Chromium 100% identique sur macOS, Windows et Linux.

| Moteur   | Rendu                         | Taille  | Maturité                    |
|----------|-------------------------------|---------|-----------------------------|
| wry/tao  | WebKit / WebView2 (natif OS)  | ~5 Mo   | Bonne                       |
| **CEF**  | **Chromium — 100% identique** | ~150 Mo | Très bonne (Spotify, Steam) |
| Electron | Chromium + Node.js            | ~250 Mo | Très bonne                  |

### 7.1 Flow de build

```
nexa build  (module.json: type=desktop, platforms=[macos,windows,linux])
   │
   ├─ [1] Compile Nexa → HTML + JS
   ├─ [2] Télécharge/cache les binaires CEF → ~/.nexa/cef/<version>/
   ├─ [3] Pour chaque platform en parallèle :
   │       Génère _nexa_out/<module>/<platform>/src/main.rs (shell CEF)
   │       cargo build --release --target <rust-target>
   └─ [4] Package :
           dist/<module>/macos/    NoteApp.app
           dist/<module>/windows/  NoteApp.exe + libcef.dll
           dist/<module>/linux/    NoteApp.AppImage
```

### 7.2 Scheme `nexa://` — assets en mémoire

CEF charge les assets via un scheme personnalisé, sans aucun serveur HTTP local.
Les fichiers HTML+JS sont embed dans le binaire via `include_dir!` au compile-time.

### 7.3 Bridge IPC — JS ↔ Rust

```javascript
// Helper injecté dans le bundle web
function _ipcInvoke(cmd, payload) {
    if (window.__NEXA_IPC__) {
        window.__NEXA_IPC__.postMessage(JSON.stringify({ cmd, payload }));
    }
}
```

```rust
// Shell CEF — dispatcher IPC
match cmd {
    "notify.show" => { /* notification native OS */ }
    "fs.read"     => { /* tokio::fs::read */ }
    "dialog.open" => { /* file picker natif */ }
    _ => {}
}
```

### 7.4 Modules `std.desktop` (futures phases)

| Classe        | Méthodes clés                              |
|---------------|--------------------------------------------|
| `Notify`      | `show(title, body)`                        |
| `Dialog`      | `openFile()`, `saveFile()`, `pickFolder()` |
| `Clipboard`   | `write(text)`, `read() => String`          |
| `Tray`        | `setIcon(path)`, `onClick(fn)`             |
| `NativeWindow`| `setTitle(s)`, `minimize()`, `maximize()`  |
| `Shell`       | `openUrl(url)`, `openPath(path)`           |

---

## 8. Package — bibliothèque réutilisable

### 8.1 Config (`module.json`)

```json
{ "name": "my-lib", "main": "lib.nx", "type": "package", "version": "1.2.0" }
```

Pas de `platforms` — un package est agnostique, compilé inline par le consommateur.

### 8.2 Source (inchangé)

```nexa
package mylib;
public class StrHelper {
    reverse(s: String) => String { return _strReverse(s); }
}
```

### 8.3 Cycle de vie

```
nexa package   → bundle .nexa (ZIP + AST + signature Ed25519)
nexa publish   → envoi au registry
nexa install   → téléchargement dans ~/.nexa/packages/<name>/<version>/
```

Le compilateur résout les imports depuis l'AST embarqué dans le bundle `.nexa`
et le compile inline selon le type/platform du module consommateur.

### 8.4 Resolver (ordre)

1. `stdlib/` — packages officiels `std.*`
2. `~/.nexa/packages/` — installés depuis le registry
3. `modules/<name>/lib/` — dépendances locales du module
4. `lib/` — dépendances projet-level

---

## 9. Impact stdlib — Double implémentation

| Type module     | Implémentation stdlib                                  |
|-----------------|--------------------------------------------------------|
| `web`/`desktop` | JS helpers existants (RUNTIME string dans `codegen.rs`) |
| `backend`/`cli` | Rust natif via crates (axum, reqwest, tokio…)          |
| `package`       | Dépend du type du module consommateur                  |

### 9.1 Disponibilité par type de module

| Module                  | web | backend | cli | desktop | package |
|-------------------------|-----|---------|-----|---------|---------|
| `std.io.Console`        | ✅  | ✅      | ✅  | ✅      | ❌      |
| `std.io.File`           | ❌  | ✅      | ✅  | ✅(IPC) | ❌      |
| `std.math.*`            | ✅  | ✅      | ✅  | ✅      | ✅      |
| `std.str.*`             | ✅  | ✅      | ✅  | ✅      | ✅      |
| `std.collections.*`     | ✅  | ✅      | ✅  | ✅      | ✅      |
| `std.async.*`           | ✅  | ✅      | ✅  | ✅      | ❌      |
| `std.net.HttpClient`    | ✅  | ✅      | ✅  | ✅      | ❌      |
| `std.net.Socket`        | ✅  | ✅      | ❌  | ✅      | ❌      |
| `std.server.HttpServer` | ❌  | ✅      | ❌  | ❌      | ❌      |
| `std.process.Process`   | ❌  | ✅      | ✅  | ❌      | ❌      |
| `std.process.Env`       | ❌  | ✅      | ✅  | ❌      | ❌      |
| `std.desktop.*`         | ❌  | ❌      | ❌  | ✅      | ❌      |

---

## 10. CLI interne — structure des fichiers

```
crates/cli/src/application/
  project.rs              ← ModuleConfig étendu (type, platforms, desktop, version)
  commands/
    build.rs              ← boucle modules existante → appelle build_module() étendu
    module.rs             ← nexa module add : accepte --type et --platforms
    init.rs               ← nexa new : génère module.json avec type+platforms
  targets/                ← nouveau dossier
    dispatcher.rs         ← effective_platforms() + Rayon par_iter
    web.rs                ← build web → bundle .nexa (actuel)
    rust.rs               ← nouveau : codegen_rust + cargo build
    desktop.rs            ← nouveau : codegen_desktop + bundle CEF
    package.rs            ← nexa package existant (étendu pour type: package)
```

**Sortie `dist/` :**
```
dist/
  <module>/
    <platform>/           ← une entrée par (module × platform)
```

---

## 11. Feuille de route — Phases

### ✅ Fait
- Web → HTML+JS → bundle `.nexa` (`nexa package` / `nexa publish` / registry)
- WASM codegen
- Stdlib complète (io, math, str, collections, async, net, server, process)
- Système de modules (`modules/<name>/module.json`)
- Build incrémental (lockfile SHA-256)

### Phase 1 — Extension `module.json` + cible Backend/CLI Rust ← prochaine étape

**Effort** : ~3-4 semaines

1. Étendre `ModuleConfig` : champs `type`, `platforms`, `desktop`, `version` + `effective_platforms()`
2. `nexa module add` : accepte `--type` et `--platforms`
3. `nexa new` : génère `module.json` avec `type`+`platforms` selon `--type`
4. `build.rs` : délègue à `build_module()` qui appelle le dispatcher
5. `targets/dispatcher.rs` : `par_iter` sur `effective_platforms()`
6. `codegen_rust.rs` : transpilation IR → Rust source
7. Implémentations stdlib Rust natives (Console, Env, Process, HttpServer/axum, HttpClient/reqwest, File/tokio)
8. Structure de sortie `dist/<module>/<platform>/`

**Rétro-compatibilité** : `type` absent dans `module.json` → comportement web actuel, sortie `dist/<module>/` inchangée.

**Résultat** : `nexa build` sur un module `backend` → binaire Rust dans `dist/api/native-linux/`

### Phase 2 — Desktop macOS (CEF)

**Effort** : ~5-6 semaines

1. Champ `desktop` dans `ModuleConfig` (title, width, height, resizable, icon)
2. Cache CEF : `~/.nexa/cef/<version>/` téléchargé au premier build desktop
3. `codegen_desktop.rs` : shell Rust + scheme `nexa://`
4. Bundle macOS : `MyApp.app` + `Info.plist` + `libcef.dylib` + resources Chromium

**Résultat** : module `desktop` avec `platforms: ["macos"]` → `MyApp.app`

### Phase 3 — Desktop Windows + Linux

**Effort** : ~2-3 semaines
- Bundling Windows : `.exe` + NSIS
- Bundling Linux : AppImage + `.deb`
- CI GitHub Actions matrix

### Phase 4 — Native Bridge + std.desktop

**Effort** : ~3 semaines
- IPC dispatcher Rust complet
- `std.desktop.Notify`, `Dialog`, `Clipboard`, `Tray`, `NativeWindow`, `Shell`

### Phase 5 — Package registry (à compléter)

`nexa publish`/`install` existent pour `web`. Étendre pour `type: package` avec versioning et résolution de dépendances transitives.

### Phase 6 — Mobile (futur)

- iOS : Swift shell + WKWebView + IPC via `WKScriptMessageHandler`
- Android : Kotlin shell + WebView + `addJavascriptInterface`

---

## 12. Questions ouvertes

| # | Question | Décision |
|---|----------|----------|
| 1 | **Backend/CLI → JS ou Rust ?** | **Rust natif** ✅ |
| 2 | **`type`/`platforms` dans `.nx` ou config ?** | **`module.json`** ✅ — cohérent avec l'existant, pas de dépendance circulaire |
| 3 | **WebView natif ou Chromium ?** | **CEF** ✅ — rendu 100% identique |
| 4 | **Auto-détection du type** | `type` absent → `web` par défaut (rétro-compatible) |
| 5 | **Cross-compilation** | Rust targets + SDK Xcode/MSVC → CI dédié |
| 6 | **Hot-reload desktop** | Dev mode CEF : rechargement auto du scheme `nexa://` |
| 7 | **Signing macOS** | Apple Developer account requis pour distribution hors App Store |
| 8 | **Package registry** | `nexa publish/install` existent, à étendre pour `type: package` |
