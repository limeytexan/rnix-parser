/// Find all attribute-path references rooted at one or more given identifiers
/// across every .nix file in a NEF pkgs/ directory.
///
/// Usage: find-refs <pkgs-dir> <root>... [--transitive]
///
///   <pkgs-dir>   Directory containing .nix package files.
///   <root>...    One or more root identifier names to search for
///                (e.g. `catalogs`, `inputs`, or both).
///   --transitive Also follow intra-directory package dependencies: if a
///                package takes an argument named `foo` and `foo.nix` exists
///                in the same directory, that file's refs are included too
///                (cycle-safe).
///
/// Each .nix file is expected to be a Nix function of the form
/// `{ arg1, catalogs, inputs, ... }: body`.  Two reference forms are
/// recognised for each root:
///
///   direct Select   root.a.b.c
///   inherit-from    inherit (root.a.b) x y;  →  root.a.b.x, root.a.b.y
use rnix::ast;
use rowan::ast::AstNode;
use std::{
    collections::{BTreeSet, HashMap, HashSet},
    env, fs,
    path::{Path, PathBuf},
};

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();

    let transitive = args.iter().any(|a| a == "--transitive");
    let positional: Vec<&str> = args.iter()
        .filter(|a| !a.starts_with("--"))
        .map(|s| s.as_str())
        .collect();

    if positional.len() < 2 {
        eprintln!("Usage: find-refs <pkgs-dir> <root>... [--transitive]");
        eprintln!("Example: find-refs pkgs/ catalogs inputs --transitive");
        std::process::exit(1);
    }

    let path = PathBuf::from(positional[0]);
    let roots: HashSet<String> = positional[1..].iter().map(|s| s.to_string()).collect();

    let (db, pkg_dir) = if path.is_dir() {
        let d = path.clone();
        (parse_dir(&path, &roots), d)
    } else {
        let d = path.parent().unwrap_or(Path::new(".")).to_path_buf();
        (parse_file(&path, &roots), d)
    };

    let found = if transitive {
        collect_transitive(db, &pkg_dir, &roots)
    } else {
        db.values().flat_map(|f| f.refs.iter().cloned()).collect()
    };

    for r in &found {
        println!("{}", r);
    }
}

// ---------------------------------------------------------------------------

#[derive(Debug)]
struct FileInfo {
    refs: BTreeSet<String>,
    /// Lambda pattern args that are not in `roots` — candidates for
    /// intra-directory transitive dependency resolution.
    dep_args: Vec<String>,
}

/// Analyze a single file and return a one-entry db keyed by its stem.
fn parse_file(path: &Path, roots: &HashSet<String>) -> HashMap<String, FileInfo> {
    let mut db = HashMap::new();
    if let Ok(content) = fs::read_to_string(path) {
        let stem = path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        db.insert(stem, analyze_file(&content, roots));
    }
    db
}

fn parse_dir(dir: &Path, roots: &HashSet<String>) -> HashMap<String, FileInfo> {
    let mut db = HashMap::new();
    let Ok(entries) = fs::read_dir(dir) else { return db };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().map_or(false, |e| e == "nix") {
            let stem = path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            if let Ok(content) = fs::read_to_string(&path) {
                db.insert(stem, analyze_file(&content, roots));
            }
        }
    }
    db
}

fn analyze_file(content: &str, roots: &HashSet<String>) -> FileInfo {
    let parse = rnix::Root::parse(content);
    let root = parse.tree();

    let mut refs = BTreeSet::new();
    let mut dep_args = Vec::new();

    // Collect pattern args from the top-level lambda.
    if let Some(rnix::ast::Expr::Lambda(lambda)) = root.expr() {
        if let Some(rnix::ast::Param::Pattern(pat)) = lambda.param() {
            for entry in pat.pat_entries() {
                if let Some(ident) = entry.ident() {
                    if let Some(name) = ident.ident_token().map(|t| t.text().to_string()) {
                        if !roots.contains(&name) {
                            dep_args.push(name);
                        }
                    }
                }
            }
        }
    }

    collect_refs(root.syntax(), &mut refs, roots);

    FileInfo { refs, dep_args }
}

/// Recursive AST walk that collects refs rooted at any name in `roots`.
///
/// Two forms are handled specially before falling through to child recursion:
///
///   `inherit (root.a.b) x y;`
///       → emits `root.a.b.x` and `root.a.b.y`, then stops descending into
///         this node (prevents the inner Select from also being emitted as the
///         bare intermediate path `root.a.b`).
///
///   `root.a.b.c`
///       → emits the full path and stops descending.
fn collect_refs(node: &rnix::SyntaxNode, refs: &mut BTreeSet<String>, roots: &HashSet<String>) {
    if let Some(inherit) = ast::Inherit::cast(node.clone()) {
        if try_handle_inherit(&inherit, refs, roots) {
            return;
        }
    }

    if let Some(select) = ast::Select::cast(node.clone()) {
        if let Some(path) = extract_ref_path(&select, roots) {
            refs.insert(path);
            return;
        }
    }

    for child in node.children() {
        collect_refs(&child, refs, roots);
    }
}

/// For `inherit (root.x.y) a b c;`, inserts `root.x.y.a` etc. and returns true.
/// Returns false when the `from` expression is not a rooted Select.
fn try_handle_inherit(
    inherit: &ast::Inherit,
    refs: &mut BTreeSet<String>,
    roots: &HashSet<String>,
) -> bool {
    let Some(from) = inherit.from() else { return false };
    let Some(from_expr) = from.expr() else { return false };
    let ast::Expr::Select(select) = from_expr else { return false };
    let Some(base_path) = extract_ref_path(&select, roots) else { return false };

    for attr in inherit.attrs() {
        if let ast::Attr::Ident(id) = attr {
            if let Some(token) = id.ident_token() {
                refs.insert(format!("{}.{}", base_path, token.text()));
            }
        }
    }
    true
}

/// Returns `Some("root.foo.bar")` when `select` has a root `Ident` whose name
/// is in `roots` and a fully static attrpath.  Returns `None` otherwise.
fn extract_ref_path(select: &ast::Select, roots: &HashSet<String>) -> Option<String> {
    let expr = select.expr()?;
    let ast::Expr::Ident(base) = expr else { return None };
    let base_name = base.ident_token()?.text().to_string();
    if !roots.contains(&base_name) {
        return None;
    }

    let attrpath = select.attrpath()?;
    let mut parts = vec![base_name];
    for attr in attrpath.attrs() {
        match attr {
            ast::Attr::Ident(id) => {
                parts.push(id.ident_token()?.text().to_string());
            }
            _ => return None, // dynamic / interpolated attr — skip
        }
    }
    Some(parts.join("."))
}

/// Transitively union refs by following intra-directory package dep_args.
/// Deps not already in `db` are lazily loaded from `dir` (tries `<name>.nix`
/// then `<name>/default.nix`).
fn collect_transitive(
    mut db: HashMap<String, FileInfo>,
    dir: &Path,
    roots: &HashSet<String>,
) -> BTreeSet<String> {
    let mut visited: HashSet<String> = HashSet::new();
    let mut result: BTreeSet<String> = BTreeSet::new();
    let mut queue: Vec<String> = db.keys().cloned().collect();

    while let Some(name) = queue.pop() {
        if !visited.insert(name.clone()) {
            continue;
        }
        if !db.contains_key(&name) {
            if let Some(info) = load_dep(dir, &name, roots) {
                db.insert(name.clone(), info);
            }
        }
        let Some(info) = db.get(&name) else { continue };
        result.extend(info.refs.iter().cloned());
        let dep_args: Vec<String> = info.dep_args.clone();
        for dep in dep_args {
            if !visited.contains(&dep) {
                queue.push(dep);
            }
        }
    }

    result
}

/// Try to load a dep file from `dir`: first `<name>.nix`, then `<name>/default.nix`.
fn load_dep(dir: &Path, name: &str, roots: &HashSet<String>) -> Option<FileInfo> {
    let candidates = [
        dir.join(format!("{}.nix", name)),
        dir.join(name).join("default.nix"),
    ];
    for path in &candidates {
        if path.is_file() {
            if let Ok(content) = fs::read_to_string(path) {
                return Some(analyze_file(&content, roots));
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn roots(names: &[&str]) -> HashSet<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    fn catalog_roots() -> HashSet<String> {
        roots(&["catalogs"])
    }

    fn input_roots() -> HashSet<String> {
        roots(&["inputs"])
    }

    fn both_roots() -> HashSet<String> {
        roots(&["catalogs", "inputs"])
    }

    fn refs(content: &str, roots: &HashSet<String>) -> BTreeSet<String> {
        analyze_file(content, roots).refs
    }

    fn set(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    // -----------------------------------------------------------------------
    // Pattern: no catalog argument — pure nixpkgs package
    // -----------------------------------------------------------------------

    #[test]
    fn no_catalog_refs_fetchpypi() {
        let got = refs(
            include_str!("../test_data/catalog_refs/no-catalog-refs.nix"),
            &catalog_roots(),
        );
        assert_eq!(got, BTreeSet::new());
    }

    #[test]
    fn no_catalog_refs_rust_package() {
        // Rust/maturin package — no `catalogs` argument at all.
        let got = refs(
            include_str!("../test_data/catalog_refs/rust-no-catalog.nix"),
            &catalog_roots(),
        );
        assert_eq!(got, BTreeSet::new());
    }

    // -----------------------------------------------------------------------
    // Pattern: non-catalog inherit — should produce zero refs for any root
    // -----------------------------------------------------------------------

    #[test]
    fn non_catalog_inherit_not_collected() {
        // `inherit (pyprojectAttrs.project) version` and
        // `inherit (buildAttrs) system` must not appear as catalog or input refs.
        let content = include_str!("../test_data/catalog_refs/non-catalog-inherit.nix");
        assert_eq!(refs(content, &catalog_roots()), BTreeSet::new());
        assert_eq!(refs(content, &input_roots()), BTreeSet::new());
    }

    // -----------------------------------------------------------------------
    // Pattern: single inherit-from for a toolkit helper function
    // -----------------------------------------------------------------------

    #[test]
    fn single_inherit_helper() {
        let got = refs(
            include_str!("../test_data/catalog_refs/single-inherit-helper.nix"),
            &catalog_roots(),
        );
        assert_eq!(got, set(&["catalogs.myorg.toolkit.readVersion"]));
    }

    // -----------------------------------------------------------------------
    // Pattern: two separate inherit-from statements
    // -----------------------------------------------------------------------

    #[test]
    fn two_inherits_toolkit_and_python_pkg() {
        let got = refs(
            include_str!("../test_data/catalog_refs/two-inherits.nix"),
            &catalog_roots(),
        );
        assert_eq!(
            got,
            set(&[
                "catalogs.myorg.toolkit.readVersion",
                "catalogs.myorg.python3Packages.beta-client",
            ])
        );
    }

    // -----------------------------------------------------------------------
    // Pattern: multi-attr inherit-from
    // -----------------------------------------------------------------------

    #[test]
    fn multi_attr_inherit_expands_all_names() {
        let got = refs(
            include_str!("../test_data/catalog_refs/multi-attr-inherit.nix"),
            &catalog_roots(),
        );
        assert_eq!(
            got,
            set(&[
                "catalogs.myorg.toolkit.readVersion",
                "catalogs.myorg.python3Packages.alpha-lib",
                "catalogs.myorg.python3Packages.delta-util",
                "catalogs.myorg.python3Packages.epsilon-core",
                "catalogs.myorg.python3Packages.eta-parser",
                "catalogs.myorg.python3Packages.theta-worker",
            ])
        );
    }

    #[test]
    fn multi_attr_inherit_no_bare_intermediate_path() {
        // The intermediate path `catalogs.myorg.python3Packages` must NOT
        // appear — only the fully-qualified per-attr paths should be present.
        let got = refs(
            include_str!("../test_data/catalog_refs/multi-attr-inherit.nix"),
            &catalog_roots(),
        );
        assert!(!got.contains("catalogs.myorg.python3Packages"));
        assert!(!got.contains("catalogs.myorg.toolkit"));
    }

    // -----------------------------------------------------------------------
    // Pattern: direct `root.org.pkg` Select for native packages
    // -----------------------------------------------------------------------

    #[test]
    fn direct_select_native_packages() {
        let got = refs(
            include_str!("../test_data/catalog_refs/direct-select-native.nix"),
            &catalog_roots(),
        );
        assert_eq!(
            got,
            set(&[
                "catalogs.myorg.toolkit.readMakeVersion",
                "catalogs.myorg.python3Packages.epsilon-core",
                "catalogs.myorg.proxy-wrap",
                "catalogs.myorg.queue-bin",
            ])
        );
    }

    // -----------------------------------------------------------------------
    // Pattern: inherit the whole sub-attrset from a root's top level
    //   inherit (catalogs.myorg) toolkit;
    // -----------------------------------------------------------------------

    #[test]
    fn inherit_whole_subattrset() {
        let got = refs(
            include_str!("../test_data/catalog_refs/inherit-subattrset.nix"),
            &catalog_roots(),
        );
        assert_eq!(got, set(&["catalogs.myorg.toolkit"]));
    }

    // -----------------------------------------------------------------------
    // Pattern: inline sub-package in let alongside catalog refs
    // -----------------------------------------------------------------------

    #[test]
    fn nested_inline_package_does_not_hide_outer_refs() {
        let got = refs(
            include_str!("../test_data/catalog_refs/nested-inline-package.nix"),
            &catalog_roots(),
        );
        assert_eq!(
            got,
            set(&[
                "catalogs.myorg.toolkit.readVersion",
                "catalogs.myorg.python3Packages.alpha-lib",
                "catalogs.myorg.python3Packages.gamma-service",
                "catalogs.myorg.python3Packages.theta-worker",
            ])
        );
    }

    // -----------------------------------------------------------------------
    // Pattern: passthru.src string-interpolation must not create extra refs
    // -----------------------------------------------------------------------

    #[test]
    fn passthru_src_interpolation_no_extra_refs() {
        let got = refs(
            include_str!("../test_data/catalog_refs/passthru-src-access.nix"),
            &catalog_roots(),
        );
        assert_eq!(
            got,
            set(&[
                "catalogs.myorg.toolkit.readVersion",
                "catalogs.myorg.python3Packages.gamma-service",
                "catalogs.myorg.python3Packages.zeta-api",
                "catalogs.myorg.queue-bin",
            ])
        );
        assert!(!got.iter().any(|r| r.contains(".src")));
    }

    // -----------------------------------------------------------------------
    // Pattern: inputs.* only — inherit-from and direct Select
    // -----------------------------------------------------------------------

    #[test]
    fn inputs_only_with_input_roots() {
        let got = refs(
            include_str!("../test_data/catalog_refs/inputs-only.nix"),
            &input_roots(),
        );
        assert_eq!(
            got,
            set(&[
                "inputs.nixpkgs.lib",
                "inputs.devtools-flake.packages.default",
                "inputs.self",
            ])
        );
    }

    #[test]
    fn inputs_only_with_catalog_roots_returns_nothing() {
        // Same file, but searching for `catalogs` — should be empty.
        let got = refs(
            include_str!("../test_data/catalog_refs/inputs-only.nix"),
            &catalog_roots(),
        );
        assert_eq!(got, BTreeSet::new());
    }

    // -----------------------------------------------------------------------
    // Pattern: mixed catalogs.* and inputs.* in one file
    // -----------------------------------------------------------------------

    #[test]
    fn mixed_roots_catalog_only() {
        let got = refs(
            include_str!("../test_data/catalog_refs/mixed-roots.nix"),
            &catalog_roots(),
        );
        assert_eq!(
            got,
            set(&[
                "catalogs.myorg.toolkit.readVersion",
                "catalogs.myorg.python3Packages.alpha-lib",
            ])
        );
    }

    #[test]
    fn mixed_roots_inputs_only() {
        let got = refs(
            include_str!("../test_data/catalog_refs/mixed-roots.nix"),
            &input_roots(),
        );
        assert_eq!(
            got,
            set(&[
                "inputs.nixpkgs.lib",
                "inputs.devtools-flake.packages.default",
            ])
        );
    }

    #[test]
    fn mixed_roots_both() {
        let got = refs(
            include_str!("../test_data/catalog_refs/mixed-roots.nix"),
            &both_roots(),
        );
        assert_eq!(
            got,
            set(&[
                "catalogs.myorg.toolkit.readVersion",
                "catalogs.myorg.python3Packages.alpha-lib",
                "inputs.nixpkgs.lib",
                "inputs.devtools-flake.packages.default",
            ])
        );
    }

    // -----------------------------------------------------------------------
    // Integration: collect_transitive across a two-file intra-dir graph
    // -----------------------------------------------------------------------

    #[test]
    fn transitive_follows_intra_dir_dep_args() {
        let r = catalog_roots();
        let file_a = "{ catalogs, beta-client }: catalogs.myorg.toolkit.readVersion";
        let file_b = "{ catalogs }: catalogs.myorg.python3Packages.gamma-service";

        let mut db = HashMap::new();
        db.insert("alpha-lib".to_string(), analyze_file(file_a, &r));
        db.insert("beta-client".to_string(), analyze_file(file_b, &r));

        let got = collect_transitive(db, Path::new("."), &r);
        assert_eq!(
            got,
            set(&[
                "catalogs.myorg.toolkit.readVersion",
                "catalogs.myorg.python3Packages.gamma-service",
            ])
        );
    }

    #[test]
    fn transitive_cycle_safe() {
        let r = catalog_roots();
        let file_a = "{ catalogs, pkg-b }: catalogs.myorg.x";
        let file_b = "{ catalogs, pkg-a }: catalogs.myorg.y";

        let mut db = HashMap::new();
        db.insert("pkg-a".to_string(), analyze_file(file_a, &r));
        db.insert("pkg-b".to_string(), analyze_file(file_b, &r));

        let got = collect_transitive(db, Path::new("."), &r);
        assert_eq!(got, set(&["catalogs.myorg.x", "catalogs.myorg.y"]));
    }

    #[test]
    fn transitive_inputs_root() {
        // Transitive resolution also works when root is `inputs`.
        let r = input_roots();
        let file_a = "{ inputs, dep-pkg }: inputs.nixpkgs.lib";
        let file_b = "{ inputs }: inputs.devtools-flake.packages.default";

        let mut db = HashMap::new();
        db.insert("main-pkg".to_string(), analyze_file(file_a, &r));
        db.insert("dep-pkg".to_string(), analyze_file(file_b, &r));

        let got = collect_transitive(db, Path::new("."), &r);
        assert_eq!(
            got,
            set(&[
                "inputs.nixpkgs.lib",
                "inputs.devtools-flake.packages.default",
            ])
        );
    }
}
