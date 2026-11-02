use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};

pub const MAX_FILES: usize = 64;
pub const MAX_PROJECT_BYTES: usize = 1_048_576;
pub const MAX_PROJECT_ASSET_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct File {
    pub path: String,
    pub content: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssetBinding {
    pub path: String,
    pub asset_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum AssetChange {
    Bind { path: String, asset_id: String },
    Unbind { path: String },
}

impl AssetChange {
    pub fn path(&self) -> &str {
        match self {
            Self::Bind { path, .. } | Self::Unbind { path } => path,
        }
    }
}

pub fn validate_project(files: &[File], bindings: &[AssetBinding]) -> Result<()> {
    validate_files(files)?;
    if files.len() + bindings.len() > MAX_FILES {
        bail!("A project must fit within 64 source files and asset bindings.");
    }
    for binding in bindings {
        if !valid_path(&binding.path) || uuid::Uuid::parse_str(&binding.asset_id).is_err() {
            bail!("Project asset bindings require portable paths and immutable asset UUIDs.");
        }
    }
    let names: Vec<_> = files
        .iter()
        .map(|file| file.path.as_str())
        .chain(bindings.iter().map(|binding| binding.path.as_str()))
        .collect();
    let paths: HashSet<_> = names.iter().map(|path| path.to_ascii_lowercase()).collect();
    if paths.len() != names.len() {
        bail!("Source files and asset bindings must have unique paths.");
    }
    for path in &paths {
        let mut prefix = path.as_str();
        while let Some((parent, _)) = prefix.rsplit_once('/') {
            if paths.contains(parent) {
                bail!("A project path cannot be both a file and a directory.");
            }
            prefix = parent;
        }
    }
    Ok(())
}

pub fn apply_project_changes(
    files: &[File],
    bindings: &[AssetBinding],
    changes: &[FileChange],
    asset_changes: &[AssetChange],
) -> Result<(Vec<File>, Vec<AssetBinding>)> {
    validate_project(files, bindings)?;
    if changes.len() + asset_changes.len() == 0 || changes.len() + asset_changes.len() > MAX_FILES {
        bail!("A proposal must contain 1–64 source or asset changes.");
    }
    let mut seen: HashSet<String> = changes
        .iter()
        .map(|change| change.path().to_ascii_lowercase())
        .collect();
    let mut next: BTreeMap<_, _> = bindings
        .iter()
        .map(|b| (b.path.clone(), b.asset_id.clone()))
        .collect();
    for change in asset_changes {
        if !valid_path(change.path()) || !seen.insert(change.path().to_ascii_lowercase()) {
            bail!("Changed source and asset paths must be unique portable paths.");
        }
        match change {
            AssetChange::Bind { path, asset_id } => {
                if uuid::Uuid::parse_str(asset_id).is_err() || next.get(path) == Some(asset_id) {
                    bail!("An asset binding must select a valid, changed asset UUID.");
                }
                next.insert(path.clone(), asset_id.clone());
            }
            AssetChange::Unbind { path } => {
                if next.remove(path).is_none() {
                    bail!("An unbind must name an existing project asset path.");
                }
            }
        }
    }
    let files = if changes.is_empty() {
        files.to_vec()
    } else {
        apply_changes(files, changes)?
    };
    let bindings: Vec<_> = next
        .into_iter()
        .map(|(path, asset_id)| AssetBinding { path, asset_id })
        .collect();
    validate_project(&files, &bindings)?;
    Ok((files, bindings))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum FileChange {
    Write { path: String, content: String },
    Delete { path: String },
}

impl FileChange {
    pub fn path(&self) -> &str {
        match self {
            Self::Write { path, .. } | Self::Delete { path } => path,
        }
    }
}

#[derive(Clone)]
pub enum GeneratedArtifact {
    Html(String),
    Changes(Vec<FileChange>),
    Workspace {
        changes: Vec<FileChange>,
        asset_changes: Vec<AssetChange>,
    },
}

pub fn valid_path(path: &str) -> bool {
    if path.is_empty()
        || path.len() > 240
        || !path
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-/".contains(&byte))
    {
        return false;
    }
    path.split('/').all(|part| {
        let name = part
            .split('.')
            .next()
            .unwrap_or_default()
            .to_ascii_uppercase();
        !part.is_empty()
            && part != "."
            && part != ".."
            && !part.ends_with('.')
            && !matches!(name.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            && !(name.len() == 4
                && (name.starts_with("COM") || name.starts_with("LPT"))
                && matches!(name.as_bytes()[3], b'1'..=b'9'))
    })
}

pub fn validate_files(files: &[File]) -> Result<()> {
    if files.len() > MAX_FILES
        || files
            .iter()
            .map(|f| f.path.len() + f.content.len())
            .sum::<usize>()
            > MAX_PROJECT_BYTES
    {
        bail!("A project must fit within 64 text files and 1 MiB.");
    }
    let mut paths = HashSet::new();
    for file in files {
        if !valid_path(&file.path) || !paths.insert(file.path.to_ascii_lowercase()) {
            bail!("Project paths must be unique portable relative file paths.");
        }
    }
    for path in &paths {
        let mut prefix = path.as_str();
        while let Some((parent, _)) = prefix.rsplit_once('/') {
            if paths.contains(parent) {
                bail!("A project path cannot be both a file and a directory.");
            }
            prefix = parent;
        }
    }
    Ok(())
}

pub fn apply_changes(base: &[File], changes: &[FileChange]) -> Result<Vec<File>> {
    validate_files(base)?;
    if changes.is_empty() || changes.len() > MAX_FILES {
        bail!("A proposal must contain 1–64 file changes.");
    }
    let mut files: BTreeMap<_, _> = base
        .iter()
        .map(|file| (file.path.clone(), file.content.clone()))
        .collect();
    let mut seen = HashSet::new();
    for change in changes {
        let path = change.path();
        if !valid_path(path) || !seen.insert(path.to_ascii_lowercase()) {
            bail!("Changed paths must be unique portable relative file paths.");
        }
        match change {
            FileChange::Write { content, .. } => {
                if files.get(path) == Some(content) {
                    bail!("A write must change the file content.");
                }
                files.insert(path.into(), content.clone());
            }
            FileChange::Delete { .. } => {
                if files.remove(path).is_none() {
                    bail!("A delete must name an existing file.");
                }
            }
        }
    }
    let files: Vec<_> = files
        .into_iter()
        .map(|(path, content)| File { path, content })
        .collect();
    validate_files(&files)?;
    Ok(files)
}

pub fn parse_changes(content: &str) -> Result<Vec<FileChange>> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Envelope {
        changes: Vec<FileChange>,
    }
    let content = content.trim();
    let json = if content.starts_with('{') {
        content
    } else {
        let start = ["```files\n", "```files\r\n", "```json\n", "```json\r\n"]
            .into_iter()
            .filter_map(|fence| content.find(fence).map(|pos| pos + fence.len()))
            .min()
            .ok_or_else(|| anyhow::anyhow!("No file-change JSON object was returned."))?;
        let tail = &content[start..];
        let end = tail
            .rmatch_indices("\n```")
            .find_map(|(pos, _)| {
                tail[pos + 4..]
                    .split('\n')
                    .next()?
                    .trim()
                    .is_empty()
                    .then_some(pos)
            })
            .ok_or_else(|| anyhow::anyhow!("The file-change JSON fence was not closed."))?;
        &tail[..end]
    };
    let envelope: Envelope = serde_json::from_str(json)
        .map_err(|_| anyhow::anyhow!("The response must contain valid file-change JSON."))?;
    Ok(envelope.changes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_containers_do_not_relax_payload_validation() {
        let json = r#"{"changes":[{"op":"write","path":"a.txt","content":"hello\nworld"}]}"#;
        let expected = parse_changes(json).unwrap();
        for fence in ["files", "json"] {
            assert_eq!(
                parse_changes(&format!("Proposal:\n```{fence}\r\n{json}\r\n```\n")).unwrap(),
                expected
            );
            assert!(parse_changes(&format!("```{fence}\n{json}")).is_err());
        }
        for invalid in [
            format!("{json} extra"),
            json.trim_end_matches('}').into(),
            json.replace("hello", "bad\"quote"),
            json.replace("\"changes\"", "\"extra\":true,\"changes\""),
        ] {
            assert!(parse_changes(&invalid).is_err());
        }
    }

    #[test]
    fn writes_deletes_and_untouched_files_form_an_exact_new_snapshot() {
        let base = vec![
            File {
                path: "keep.txt".into(),
                content: "exact\r\n界🙂".into(),
            },
            File {
                path: "old.txt".into(),
                content: "remove".into(),
            },
        ];
        let changes = parse_changes("```files\n{\"changes\":[{\"op\":\"delete\",\"path\":\"old.txt\"},{\"op\":\"write\",\"path\":\"src/app.js\",\"content\":\"\"}]}\n```").unwrap();
        let result = apply_changes(&base, &changes).unwrap();
        assert_eq!(result[0], base[0]);
        assert_eq!(result[1].path, "src/app.js");
        assert_eq!(result[1].content, "");
        assert_eq!(base.len(), 2);
    }

    #[test]
    fn ambiguous_or_host_paths_are_rejected() {
        for path in [
            "",
            "/root",
            "../file",
            "src/../file",
            "C:/file",
            "src\\file",
            "a//b",
            "a/",
            "CON.txt",
            "lpt1",
            "file.",
            "a%2fb",
            "x\0y",
        ] {
            assert!(!valid_path(path), "{path:?}");
        }
        for paths in [["a", "A"], ["src", "src/app.js"]] {
            let files: Vec<_> = paths
                .into_iter()
                .map(|path| File {
                    path: path.into(),
                    content: String::new(),
                })
                .collect();
            assert!(validate_files(&files).is_err());
        }
        assert!(valid_path("src/.config-file_1.ts"));
    }

    #[test]
    fn malformed_noop_missing_and_oversized_changes_fail() {
        assert!(
            parse_changes("```files\n{\"changes\":[{\"op\":\"write\",\"path\":\"a\"}]}\n```")
                .is_err()
        );
        assert!(apply_changes(&[], &[]).is_err());
        assert!(
            apply_changes(
                &[],
                &[FileChange::Delete {
                    path: "missing".into()
                }]
            )
            .is_err()
        );
        assert!(
            apply_changes(
                &[],
                &[FileChange::Write {
                    path: "a".into(),
                    content: "x".repeat(MAX_PROJECT_BYTES)
                }]
            )
            .is_err()
        );
        let base = vec![File {
            path: "a".into(),
            content: "same".into(),
        }];
        assert!(
            apply_changes(
                &base,
                &[FileChange::Write {
                    path: "a".into(),
                    content: "same".into()
                }]
            )
            .is_err()
        );
    }
}
