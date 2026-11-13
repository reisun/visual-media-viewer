use serde::{Deserialize, Serialize};
use std::io::{self, Read};
use std::path::PathBuf;

pub fn deployment_matches(files: &[(PathBuf, PathBuf)]) -> io::Result<bool> {
    for (source, destination) in files {
        let mut original = std::fs::File::open(source)?;
        let mut installed = match std::fs::File::open(destination) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error),
        };
        let mut remaining = original.metadata()?.len();
        if remaining != installed.metadata()?.len() {
            return Ok(false);
        }
        let mut left = [0u8; 65536];
        let mut right = [0u8; 65536];
        while remaining > 0 {
            let count = remaining.min(left.len() as u64) as usize;
            original.read_exact(&mut left[..count])?;
            installed.read_exact(&mut right[..count])?;
            if left[..count] != right[..count] {
                return Ok(false);
            }
            remaining -= count as u64;
        }
        if original.read(&mut left[..1])? != 0 || installed.read(&mut right[..1])? != 0 {
            return Ok(false);
        }
    }
    Ok(true)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryValue {
    pub kind: u32,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Backup {
    pub original: Option<RegistryValue>,
}

pub fn may_remove_app_class(own_command: bool, effective_default: bool, user_choice: bool) -> bool {
    own_command && !effective_default && !user_choice
}

pub fn rollback_changes(
    changes: &[(String, String, Option<RegistryValue>)],
    mut write: impl FnMut(&str, &str, Option<&RegistryValue>) -> Result<(), String>,
) -> Vec<String> {
    changes
        .iter()
        .rev()
        .filter_map(|(path, name, value)| write(path, name, value.as_ref()).err())
        .collect()
}

pub fn restore_value(
    current: Option<&RegistryValue>,
    ours: &RegistryValue,
    backup: Option<&Backup>,
) -> Option<Option<RegistryValue>> {
    (current == Some(ours)).then(|| backup.and_then(|saved| saved.original.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value(bytes: &[u8]) -> RegistryValue {
        RegistryValue {
            kind: 1,
            bytes: bytes.to_vec(),
        }
    }

    #[test]
    fn restores_absent_and_empty_values_distinctly() {
        let ours = value(b"ours");
        assert_eq!(
            restore_value(Some(&ours), &ours, Some(&Backup { original: None })),
            Some(None)
        );
        let empty = value(b"");
        assert_eq!(
            restore_value(
                Some(&ours),
                &ours,
                Some(&Backup {
                    original: Some(empty.clone())
                })
            ),
            Some(Some(empty))
        );
    }

    #[test]
    fn preserves_changes_made_by_other_apps() {
        let ours = value(b"ours");
        let other = value(b"new-app");
        let saved = Backup {
            original: Some(value(b"old-app")),
        };
        assert_eq!(restore_value(Some(&other), &ours, Some(&saved)), None);
        assert_eq!(restore_value(None, &ours, Some(&saved)), None);
    }

    #[test]
    fn untracked_own_registration_can_be_removed() {
        let ours = value(b"ours");
        assert_eq!(restore_value(Some(&ours), &ours, None), Some(None));
    }

    #[test]
    fn backup_roundtrip_preserves_original_registry_type_and_bytes() {
        let saved = Backup {
            original: Some(RegistryValue {
                kind: 3,
                bytes: vec![0, 255, 0, 128],
            }),
        };
        let persisted = serde_json::to_string(&saved).unwrap();
        let loaded: Backup = serde_json::from_str(&persisted).unwrap();
        assert_eq!(loaded.original, saved.original);
    }

    #[test]
    fn unregister_preserves_commands_still_used_as_defaults() {
        assert!(!may_remove_app_class(true, true, false));
        assert!(!may_remove_app_class(true, false, true));
        assert!(!may_remove_app_class(false, false, false));
        assert!(may_remove_app_class(true, false, false));
    }

    #[test]
    fn deployment_reuse_requires_identical_contents_and_all_required_files() {
        let root = std::env::temp_dir().join(format!(
            "vmv-shell-reuse-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let source = root.join("source");
        let installed = root.join("installed");
        let provider = root.join("provider");
        std::fs::write(&source, b"abcd").unwrap();
        std::fs::write(&installed, b"abcd").unwrap();
        let files = vec![(source.clone(), installed.clone())];
        assert!(deployment_matches(&files).unwrap());
        std::fs::write(&installed, b"abce").unwrap();
        assert!(!deployment_matches(&files).unwrap());
        std::fs::write(&installed, b"abcd").unwrap();
        assert!(!deployment_matches(&[
            (source.clone(), installed.clone()),
            (source.clone(), provider.clone())
        ])
        .unwrap());
        std::fs::write(&provider, b"abcd").unwrap();
        assert!(deployment_matches(&[
            (source.clone(), installed.clone()),
            (source.clone(), provider.clone())
        ])
        .unwrap());
        for file in [source, installed, provider] {
            std::fs::remove_file(file).unwrap();
        }
        std::fs::remove_dir(root).unwrap();
    }

    #[test]
    fn rollback_restores_values_in_reverse_order_and_continues_after_failure() {
        let changes = vec![
            ("backup".into(), "jpg".into(), None),
            ("handler".into(), "".into(), Some(value(b"previous"))),
            ("failed".into(), "".into(), None),
        ];
        let mut restored = Vec::new();
        let errors = rollback_changes(&changes, |path, name, original| {
            restored.push((path.to_owned(), name.to_owned(), original.cloned()));
            if path == "failed" {
                Err("access denied".into())
            } else {
                Ok(())
            }
        });
        assert_eq!(errors, vec!["access denied"]);
        assert_eq!(restored[0].0, "failed");
        assert_eq!(restored[1].2, Some(value(b"previous")));
        assert_eq!(restored[2].2, None);
        assert_eq!(restored[2].0, "backup");
    }
}
