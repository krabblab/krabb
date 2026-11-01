use std::{fs, time::Duration, time::SystemTime};

pub struct WriteFixture {
    pub root: std::path::PathBuf,
    protected: bool,
}

impl WriteFixture {
    pub fn new(case: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("krabb-cache-write-{case}-{}", std::process::id()));

        fs::create_dir_all(root.join("pinned")).unwrap();

        let root = fs::canonicalize(root).unwrap();

        fs::write(root.join("pinned/image"), b"pinned").unwrap();

        let protected = case.starts_with("protected");
        let prefix = if protected { ".transaction" } else { "cached" };

        if case.ends_with("bytes") {
            let file = fs::File::create(root.join(prefix)).unwrap();

            file.set_len(64 * 1024 * 1024).unwrap();
        } else {
            for index in 0..256 {
                fs::write(root.join(format!("{prefix}-{index}")), []).unwrap();
            }
        }

        Self { root, protected }
    }

    pub fn verify(&self, result: krabb_core::Result<serde_json::Value>, name: &str, bytes: &[u8]) {
        let destination = self.root.join(name);

        if self.protected {
            let error = result.unwrap_err();

            assert!(error.to_string().contains("media cache is unavailable"));
            assert!(!destination.exists(), "failed reservation published incoming media");

            if self.root.join(".transaction").exists() {
                assert_eq!(fs::metadata(self.root.join(".transaction")).unwrap().len(), 64 << 20);
            } else {
                for index in 0..256 {
                    assert!(self.root.join(format!(".transaction-{index}")).is_file());
                }
            }
        } else {
            let value = result.unwrap();

            assert_eq!(value["uri"], format!("file://{}", destination.display()));
            assert_eq!(fs::read(destination).unwrap(), bytes);
        }

        let files = fs::read_dir(&self.root)
            .unwrap()
            .map(|entry| entry.unwrap().metadata().unwrap())
            .filter(|metadata| metadata.is_file())
            .collect::<Vec<_>>();

        assert!(files.len() <= 256, "incoming media exceeded the file budget");
        assert!(files.iter().map(|metadata| metadata.len()).sum::<u64>() <= 64 << 20);
        assert_eq!(fs::read(self.root.join("pinned/image")).unwrap(), b"pinned");
    }
}

impl Drop for WriteFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

pub fn check(reserve: fn(&std::path::Path, u64) -> krabb_core::Result<()>) {
    let root = std::env::temp_dir().join(format!("krabb-channel-cache-{}", std::process::id()));

    fs::create_dir_all(root.join("pinned")).unwrap();
    fs::write(root.join("pinned/retained.bin"), "retained").unwrap();

    for index in 0..260 {
        fs::write(root.join(format!("{index}.bin")), []).unwrap();
    }

    reserve(&root, 9).unwrap();

    let files = fs::read_dir(&root)
        .unwrap()
        .map(Result::unwrap)
        .filter(|entry| entry.file_type().unwrap().is_file())
        .collect::<Vec<_>>();

    assert_eq!(files.len(), 255);

    for file in files {
        fs::remove_file(file.path()).unwrap();
    }

    let now = SystemTime::now();

    for index in 0..3 {
        let file = fs::File::create(root.join(format!("large-{index}.bin"))).unwrap();

        file.set_len(32 * 1024 * 1024).unwrap();
        file.set_times(fs::FileTimes::new().set_modified(now - Duration::from_secs(3 - index)))
            .unwrap();
    }

    reserve(&root, 4 * 1024 * 1024).unwrap();

    assert!(!root.join("large-0.bin").exists());
    assert!(!root.join("large-1.bin").exists());
    assert!(root.join("large-2.bin").is_file());
    assert_eq!(fs::read_to_string(root.join("pinned/retained.bin")).unwrap(), "retained");

    let destination = root.join("incoming.bin");

    krabb_file::save(&destination, b"accepted").unwrap();

    assert_eq!(fs::read(destination).unwrap(), b"accepted");
    assert!(reserve(&root.join("incoming.bin"), 1).is_err());

    fs::remove_dir_all(root).unwrap();
}
