use super::*;
use krabb_core::scope::Access;

pub(super) fn call(
    policy: &Policy,
    request: &serde_json::Value,
) -> krabb_core::Result<serde_json::Value> {
    let access: Access = serde_json::from_value(request["access"].clone())
        .map_err(|_| denied("Invalid file access."))?;
    access.validate().map_err(denied)?;
    let method = request["method"].as_str().ok_or_else(|| denied("Missing file operation."))?;
    let params = &request["params"];
    let workspace = workspace(params["workspace"].as_str())?;
    let root = file_at(policy, ".", workspace.as_deref())?;
    let home = std::env::var_os("KRABB_HOME")
        .map(PathBuf::from)
        .ok_or_else(|| denied("KRABB_HOME is required for file visibility."))?;
    let mut catalog = krabb_file::Ownership::open(&home, &root)?;
    let path = file_at(policy, params["path"].as_str().unwrap_or("."), Some(&root))?;
    let relative = path.strip_prefix(&root).map_err(|_| denied("File leaves workspace."))?;
    let own = String::from(access.owner.source.clone());
    let visible = |owner: &str| {
        owner == "global"
            || owner == own
            || access.sources.iter().any(|source| {
                String::from(source.clone()) == owner && !access.excluded.contains(source)
            })
    };

    match method {
        "read" => {
            if !catalog.owner(relative).is_some_and(visible) {
                return Err(denied("File is not visible to this integration."));
            }

            let file = confined(policy, path.to_string_lossy().as_ref(), Some(&root), false)?;
            Ok(json!({"text":read_confined(&file)?}))
        }

        "write" => {
            if params["approve"] != true {
                return Err(denied("Write approval is required."));
            }

            let text = params["text"].as_str().ok_or_else(|| denied("write.text is required"))?;
            let mut destination = relative.to_path_buf();

            for suffix in 0..=1000 {
                let owned = catalog.owner(&destination) == Some(own.as_str());

                if owned
                    || (catalog.owner(&destination).is_none()
                        && !root.join(&destination).try_exists()?)
                {
                    let file = confined(
                        policy,
                        destination.to_string_lossy().as_ref(),
                        Some(&root),
                        true,
                    )?;
                    catalog.assign(&destination, &own)?;
                    write_confined(&file, text)?;
                    return Ok(json!({"ok":true,"path":destination}));
                }

                let name = relative
                    .file_name()
                    .ok_or_else(|| denied("write.path must name a file."))?
                    .to_string_lossy();
                destination = relative.with_file_name(format!("{name}.{own}.{}", suffix + 1));
            }

            Err(denied("No unused filename is available."))
        }

        "list" => {
            let mut items = std::collections::BTreeSet::new();

            for (entry, _) in catalog.entries().filter(|(_, owner)| visible(owner)) {
                if let Ok(child) = Path::new(entry).strip_prefix(relative)
                    && let Some(name) = child.components().next()
                    && root.join(entry).is_file()
                {
                    items.insert(name.as_os_str().to_string_lossy().into_owned());
                }

                if items.len() > ENTRY_LIMIT {
                    return Err(denied("List exceeds the entry limit."));
                }
            }

            Ok(json!({"items":items}))
        }

        "search" => {
            let needle = params["text"]
                .as_str()
                .filter(|s| !s.is_empty())
                .ok_or_else(|| denied("search.text is required"))?;
            let mut hits = Vec::new();
            let mut budget = 0usize;
            let start = std::time::Instant::now();

            for (entry, owner) in catalog.entries() {
                if !visible(owner) || !Path::new(entry).starts_with(relative) {
                    continue;
                }

                if start.elapsed() > SEARCH_TIME {
                    return Err(denied("Search exceeded the time limit."));
                }

                if !root.join(entry).try_exists()? {
                    continue;
                }

                let file = confined(policy, entry, Some(&root), false)?;
                let text = match read_confined(&file) {
                    Ok(text) => text,
                    Err(error)
                        if matches!(
                            error.kind(),
                            std::io::ErrorKind::InvalidData | std::io::ErrorKind::NotFound
                        ) =>
                    {
                        continue;
                    }

                    Err(error) => return Err(error.into()),
                };

                budget = budget.saturating_add(text.len());

                if budget > SEARCH_BYTES as usize {
                    return Err(denied("Search exceeds the byte limit."));
                }

                for (line, text) in text.lines().enumerate() {
                    if text.contains(needle) {
                        hits.push(format!("{entry}:{}", line + 1));

                        if hits.len() >= 100 {
                            return Ok(json!({"hits":hits}));
                        }
                    }
                }
            }

            Ok(json!({"hits":hits}))
        }

        _ => Err(denied("Unsupported visible file operation.")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use krabb_core::scope::{Owner, Principal};

    #[test]
    fn file_grants_filter_reads_listings_searches_and_protect_foreign_writes() {
        let home = std::env::temp_dir().join(format!("krabb-visible-files-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(home.join("workspace")).unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "visibility::tests::worker", "--ignored", "--nocapture"])
            .env("KRABB_HOME", &home)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    #[ignore = "isolated environment fixture"]
    fn worker() {
        let home = PathBuf::from(std::env::var_os("KRABB_HOME").unwrap());
        let root = home.join("workspace");
        let policy = Policy { root: Some(root.clone()), shell: Shell::Off };
        let access = |id: &str, sources: Vec<Principal>| Access {
            owner: Owner { source: Principal::Channel(id.into()), conversation: id.into() },
            sources,
            excluded: vec![],
        };

        let telegram = access("telegram", vec![]);
        let discord = access("discord", vec![telegram.owner.source.clone()]);
        let slack = access("slack", vec![discord.owner.source.clone()]);
        let request = |access: &Access, method: &str, params: serde_json::Value| json!({"access":access,"method":method,"params":params});
        let write = |text: &str| json!({"path":"note.txt","text":text,"approve":true});

        assert!(
            call(&policy, &request(&telegram, "write", json!({"path":"note.txt","text":"denied"})))
                .is_err()
        );

        assert_eq!(
            call(&policy, &request(&telegram, "write", write("telegram secret"))).unwrap()["path"],
            "note.txt"
        );

        assert_eq!(
            call(&policy, &request(&discord, "read", json!({"path":"note.txt"}))).unwrap()["text"],
            "telegram secret"
        );

        assert!(call(&policy, &request(&slack, "read", json!({"path":"note.txt"}))).is_err());
        assert!(
            call(&policy, &request(&slack, "list", json!({}))).unwrap()["items"]
                .as_array()
                .unwrap()
                .is_empty()
        );

        assert!(
            call(&policy, &request(&slack, "search", json!({"text":"secret"}))).unwrap()["hits"]
                .as_array()
                .unwrap()
                .is_empty()
        );

        assert_eq!(
            call(&policy, &request(&discord, "search", json!({"text":"secret"}))).unwrap()["hits"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        let collision = call(&policy, &request(&discord, "write", write("discord note"))).unwrap();

        assert_ne!(collision["path"], "note.txt");
        assert_eq!(std::fs::read_to_string(root.join("note.txt")).unwrap(), "telegram secret");
        assert!(
            call(&policy, &request(&telegram, "read", json!({"path":collision["path"]}))).is_err()
        );

        assert!(
            call(&policy, &request(&telegram, "read", json!({"path":"../data/system/KRAB.md"})))
                .is_err()
        );

        assert!(call(&policy, &request(&telegram, "shell", json!({}))).is_err());

        std::fs::write(root.join("manual.txt"), "global reference").unwrap();

        assert!(call(&policy, &request(&telegram, "read", json!({"path":"manual.txt"}))).is_err());
        krabb_file::Ownership::open(&home, &root)
            .unwrap()
            .assign(Path::new("manual.txt"), "global")
            .unwrap();
        assert_eq!(
            call(&policy, &request(&slack, "read", json!({"path":"manual.txt"}))).unwrap()["text"],
            "global reference"
        );

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(home.join("data"), root.join("escape")).unwrap();

            assert!(
                call(
                    &policy,
                    &request(
                        &telegram,
                        "write",
                        json!({"path":"escape/secret","text":"x","approve":true})
                    )
                )
                .is_err()
            );
        }
    }
}
