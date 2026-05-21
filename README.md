# nef-catalog-refs

Static analyzer for NEF (Nix Expression Format) package files. Scans `.nix`
package expressions and reports every `catalogs.*` (or other root) attribute-path
reference — without running the Nix evaluator.

Built on top of [rnix-parser](https://github.com/nix-community/rnix-parser), a
Rust CST/AST library for Nix.

## find-refs

```
find-refs <pkgs-dir-or-file> <root>... [--transitive]
```

| Argument | Description |
|---|---|
| `<pkgs-dir-or-file>` | A NEF `pkgs/` directory or a single `.nix` file |
| `<root>` | Root name(s) to search for — e.g. `catalogs`, `inputs`, or both |
| `--transitive` | Also follow intra-directory package dep args (cycle-safe) |

Output is a sorted, deduplicated list of `<root>.<path>` strings, one per line.

### Reference forms handled

| Form | Nix example | Output |
|---|---|---|
| Direct select | `catalogs.myorg.pkg` | `catalogs.myorg.pkg` |
| inherit-from | `inherit (catalogs.myorg.toolkit) readVersion;` | `catalogs.myorg.toolkit.readVersion` |
| with expression | `with catalogs.myorg; ...` | `catalogs.myorg.*` (conservative) |
| let alias | `let org = catalogs.myorg; in org.pkg` | `catalogs.myorg`, `catalogs.myorg.pkg` |
| Dynamic attr | `catalogs.myorg.${name}` | `catalogs.myorg.*` |
| builtins.getAttr | `builtins.getAttr "pkg" catalogs.myorg` | `catalogs.myorg.pkg` |
| Cross-file import | `import ./helper.nix { inherit catalogs; }` | refs unioned from helper file |

## Building

### With flakes

```sh
nix build
./result/bin/find-refs pkgs/ catalogs --transitive
```

### Without flakes

```sh
nix-build
./result/bin/find-refs pkgs/ catalogs --transitive
```

### With Cargo

```sh
cargo build -p nef-catalog-refs --bin find-refs
cargo test -p nef-catalog-refs
```

## Development shell

```sh
nix develop      # provides rustc, cargo, clippy, rustfmt
```

## Library usage

`nef-catalog-refs` exposes a public API for embedding the analysis in other Rust
programs:

```rust
use nef_catalog_refs::{parse_dir, collect_transitive};
use std::collections::HashSet;
use std::path::Path;

let roots: HashSet<String> = ["catalogs".to_string()].into();
let db = parse_dir(Path::new("pkgs/"), &roots);
let refs = collect_transitive(db, Path::new("pkgs/"), &roots);
for r in &refs {
    println!("{}", r);
}
```

## License

MIT
