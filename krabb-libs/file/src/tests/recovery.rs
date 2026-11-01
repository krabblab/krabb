use crate::*;

#[test]
fn recovery_preserves_files_referenced_by_invalid_journals() {
    let root = tests::root("unsafe-journals");
    let directory = root.join("state");

    fs::create_dir(&directory).unwrap();

    let path = directory.join("state.json");
    let journal = directory.join(".state.json.txn-test");
    let sibling = root.join("sentinel");
    let neighbor = directory.join("sentinel");
    let other = directory.join(".state.json.tmp-other");

    let references = [
        "../sentinel".to_owned(),
        sibling.to_str().unwrap().to_owned(),
        "state.json".to_owned(),
        "./state.json".to_owned(),
        "sentinel".to_owned(),
        ".state.json.txn-test".to_owned(),
        ".state.json.tmp-other".to_owned(),
        ".state.json.tmp-test".to_owned(),
        ".state.json.backup-test".to_owned(),
        r"..\sentinel".to_owned(),
        r"C:\sentinel".to_owned(),
        r"C:sentinel".to_owned(),
        r"\\server\share\sentinel".to_owned(),
        String::new(),
    ];

    for phase in ["prepared", "backed_up", "installed"] {
        for target_exists in [false, true] {
            for field in ["temporary", "backup"] {
                for reference in &references {
                    let valid = if field == "temporary" {
                        ".state.json.tmp-test"
                    } else {
                        ".state.json.backup-test"
                    };

                    if reference == valid {
                        continue;
                    }

                    fs::write(&sibling, b"sibling").unwrap();
                    fs::write(&neighbor, b"neighbor").unwrap();
                    fs::write(&other, b"other transaction").unwrap();

                    if target_exists {
                        save(&path, b"current").unwrap();
                    }

                    let mut transaction = serde_json::json!({
                        "path": "state.json", "phase": phase,
                        "temporary": ".state.json.tmp-test",
                        "backup": ".state.json.backup-test"
                    });

                    transaction[field] = reference.clone().into();

                    let bytes = serde_json::to_vec(&transaction).unwrap();

                    fs::write(&journal, &bytes).unwrap();

                    let result = load(&path, 64);

                    assert_eq!(fs::read(&sibling).unwrap(), b"sibling", "{transaction}");
                    assert_eq!(fs::read(&neighbor).unwrap(), b"neighbor", "{transaction}");
                    assert_eq!(fs::read(&other).unwrap(), b"other transaction", "{transaction}");

                    if target_exists {
                        assert_eq!(fs::read(&path).unwrap(), b"current", "{transaction}");

                        fs::remove_file(&path).unwrap();
                    } else {
                        assert!(!path.exists(), "{transaction}");
                    }

                    assert_eq!(result.unwrap_err().kind(), io::ErrorKind::InvalidData);
                    assert_eq!(fs::read(&journal).unwrap(), bytes);

                    fs::remove_file(&journal).unwrap();
                }
            }
        }
    }

    fs::remove_dir_all(root).unwrap();
}
