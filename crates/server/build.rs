use std::{
    env, fs,
    path::{Path, PathBuf},
};

const FRAGMENTS: &[&str] = &[
    "app.md",
    "chat.md",
    "common.md",
    "compute.md",
    "files.md",
    "image.md",
    "import.md",
    "node.md",
    "search.md",
    "workspace.md",
];

const TIERS: &[&str] = &["models", "families", "providers"];

fn sorted_entries(dir: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", dir.display()))
        .map(|entry| entry.unwrap().path())
        .collect();
    paths.sort();
    paths
}

fn valid_key(key: &str) -> bool {
    !key.is_empty()
        && key
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b))
}

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let prompts_dir = manifest_dir.join("src").join("prompts");
    println!("cargo:rerun-if-changed={}", prompts_dir.display());

    let default_dir = prompts_dir.join("default");
    println!("cargo:rerun-if-changed={}", default_dir.display());
    for fragment in FRAGMENTS {
        let path = default_dir.join(fragment);
        assert!(
            path.is_file(),
            "missing default prompt fragment: {}",
            path.display()
        );
        println!("cargo:rerun-if-changed={}", path.display());
    }

    let mut arms = String::new();
    for tier in TIERS {
        let tier_dir = prompts_dir.join(tier);
        println!("cargo:rerun-if-changed={}", tier_dir.display());
        if !tier_dir.exists() {
            continue;
        }
        for key_dir in sorted_entries(&tier_dir) {
            if !key_dir.is_dir() {
                continue;
            }
            println!("cargo:rerun-if-changed={}", key_dir.display());
            let key = key_dir.file_name().unwrap().to_str().unwrap();
            assert!(
                valid_key(key),
                "prompt override directory '{}' in {} must be lowercase ASCII [a-z0-9._-]+",
                key,
                tier_dir.display()
            );
            for file in sorted_entries(&key_dir) {
                if !file.is_file() {
                    continue;
                }
                let name = file.file_name().unwrap().to_str().unwrap().to_owned();
                if name == "README.md" || name.starts_with('.') {
                    continue;
                }
                assert!(
                    FRAGMENTS.contains(&name.as_str()),
                    "unknown prompt fragment '{}' in {}; expected one of {:?}",
                    name,
                    key_dir.display(),
                    FRAGMENTS
                );
                println!("cargo:rerun-if-changed={}", file.display());
                let full_path = file.canonicalize().unwrap_or(file);
                arms.push_str(&format!(
                    "        ({tier:?}, {key:?}, {name:?}) => Some(include_str!({full_path:?}).trim_end()),\n"
                ));
            }
        }
    }

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let code = if arms.is_empty() {
        "fn lookup_override(_: &str, _: &str, _: &str) -> Option<&'static str> { None }\n"
            .to_owned()
    } else {
        format!(
            "fn lookup_override(tier: &str, key: &str, fragment: &str) -> Option<&'static str> {{\n    match (tier, key, fragment) {{\n{arms}        _ => None,\n    }}\n}}\n"
        )
    };
    fs::write(out_dir.join("prompt_overrides.rs"), code).unwrap();
}
