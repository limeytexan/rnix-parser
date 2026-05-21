use nef_catalog_refs::{collect_transitive, parse_dir, parse_file};
use std::{collections::HashSet, path::{Path, PathBuf}};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

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
