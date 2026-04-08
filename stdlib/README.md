# Nexa Standard Library (`std`)

Skeleton implementation of the Nexa standard library.

## Modules

| Module | File | Classes | Description |
|---|---|---|---|
| `std.io` | `src/io.nx` | `Print` | Console logging: `log`, `logInt`, `logBool` |
| `std.math` | `src/math.nx` | `Math` | `abs`, `max`, `min`, `clamp`, `isEven`, `isOdd` |
| `std.str` | `src/str.nx` | `Str` | `isEmpty`, `concat`, `repeat` |
| `std.option` | `src/option.nx` | `Option` | Optional value: `isSome`, `isNone`, `unwrapOr` |
| `std.result` | `src/result.nx` | `Result` | `isOk`, `isErr`, `unwrapOr`, `errorMessage` |

## Usage

```nexa
import std.math.Math;
import std.str.Str;
import std.io.Print;

app MyApp {
    class Calculator {
        run() => Void {
            let m = Math();
            let result: Int = m.max(10, 20);
            let p = Print();
            p.logInt(result);
        }
    }
    route "/" => HomeWindow;
}
```

## Status

These are **skeleton implementations** — the method bodies contain minimal Nexa code.
Full runtime implementations (calling into the JS/WASM host) will be added in v0.3.

### Planned additions (v0.3+)
- `std.collections` — `List<T>`, `Map<K, V>`
- `std.http` — HTTP client
- `std.json` — JSON encode/decode
- `std.time` — Date/time utilities
