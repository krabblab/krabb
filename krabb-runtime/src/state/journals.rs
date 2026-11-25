use super::*;

pub(super) fn directory(index: &Path, id: &str) -> std::io::Result<PathBuf> {
    if !valid(id) || !is_task_session(id) {
        return Err(std::io::Error::other("Invalid task journal identity."));
    }

    let home = index
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .ok_or_else(|| std::io::Error::other("Invalid session catalog."))?;
    krabb_file::directory(home, &Path::new("data/tasks").join(id).join("records"))
}

pub(super) fn cleanup(index: &Path, sessions: &BTreeMap<String, String>) -> std::io::Result<()> {
    let home = index
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .ok_or_else(|| std::io::Error::other("Invalid session catalog."))?;
    let root = krabb_file::directory(home, Path::new("data/tasks"))?;

    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let id = entry.file_name().to_string_lossy().into_owned();

        if !entry.file_type()?.is_dir() || !valid(&id) || !is_task_session(&id) {
            continue;
        }

        if let Some(key) = sessions.get(&id) {
            retain(index, &id, key)?;
        } else {
            let records = directory(index, &id)?;

            remove_session_records(&records, &id)?;
            let _ = std::fs::remove_dir(&records);
            let _ = std::fs::remove_dir(entry.path());
        }
    }

    Ok(())
}

/// Removes a journal's superseded records, keeping its current `key`.
pub(super) fn retain(index: &Path, id: &str, key: &str) -> std::io::Result<()> {
    cleanup_session_records(&directory(index, id)?, &BTreeMap::from([(id.into(), key.into())]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_task_journals_round_trip_outside_shared_conversation_records() {
        let root = crate::tests::test_root("task-journals");
        let path = root.join("data/sessions/index.json");
        let mut store = Store::load(&path).unwrap();
        store.ensure_task("task-1", "model").unwrap();
        store
            .push(
                "task-1",
                Message {
                    id: "private-context".into(),
                    session: "task-1".into(),
                    role: Role::User,
                    sender: None,
                    content: vec![krabb_core::Content::Text {
                        text: "A private report draft".into(),
                    }],
                },
            )
            .unwrap();
        let recovered = Store::load(&path).unwrap();
        let index: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let key = index["sessions"]["task-1"].as_str().unwrap();

        assert_eq!(index["version"], 16);
        assert_eq!(index["journals"], serde_json::json!(["task-1"]));
        assert!(root.join(format!("data/tasks/task-1/records/{key}.json")).is_file());
        assert!(!root.join(format!("data/sessions/records/{key}.json")).exists());
        assert_eq!(
            recovered.sessions["task-1"].context()[0].content[0].render(),
            "A private report draft"
        );
    }

    #[test]
    fn journal_retirement_removes_owned_records_and_preserves_foreign_files() {
        let root = crate::tests::test_root("task-journal-retention");
        let path = root.join("data/sessions/index.json");
        let mut store = Store::load(&path).unwrap();
        store.ensure_task("task-1", "model").unwrap();
        let records = directory(&path, "task-1").unwrap();
        std::fs::write(records.join("notes.txt"), "keep these notes").unwrap();
        std::fs::write(records.join("foreign.json"), "{\"id\":\"someone-else\"}").unwrap();
        store.sessions.remove("task-1");
        store.save().unwrap();

        assert_eq!(std::fs::read_to_string(records.join("notes.txt")).unwrap(), "keep these notes");
        assert!(records.join("foreign.json").is_file());
        assert_eq!(std::fs::read_dir(records).unwrap().count(), 2);
        assert!(Store::load(&path).unwrap().sessions.is_empty());
        assert!(directory(&path, "../task-1").is_err());
        assert!(directory(&path, "conversation").is_err());
    }

    #[test]
    fn task_journals_are_rescanned_only_when_the_set_of_journals_changes() {
        let root = crate::tests::test_root("task-journal-sweep");
        let path = root.join("data/sessions/index.json");
        let mut store = Store::load(&path).unwrap();

        store.ensure_task("task-1", "model").unwrap();

        let orphan = root.join("data/tasks/task-2/records");

        std::fs::create_dir_all(&orphan).unwrap();
        store.save().unwrap();

        assert!(orphan.is_dir());

        store.ensure_task("task-3", "model").unwrap();

        assert!(!orphan.exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_cannot_redirect_private_task_context_outside_the_home() {
        let root = crate::tests::test_root("task-journal-symlink");
        let path = root.join("data/sessions/index.json");
        let mut store = Store::load(&path).unwrap();
        let outside = crate::tests::test_root("task-journal-outside");
        std::fs::create_dir_all(&outside).unwrap();
        let tasks = root.join("data/tasks");

        if tasks.exists() {
            std::fs::remove_dir(&tasks).unwrap();
        }

        std::os::unix::fs::symlink(&outside, &tasks).unwrap();

        assert!(store.ensure_task("task-1", "model").is_err());
        assert!(!store.sessions.contains_key("task-1"));
        assert_eq!(std::fs::read_dir(outside).unwrap().count(), 0);
    }
}
