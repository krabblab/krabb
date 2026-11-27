use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
};

/// Resolves a bare program name the way a Windows shell would, so `npx` finds `npx.cmd`.
/// Other platforms and names with a path or an extension are returned unchanged.
pub fn program(name: impl AsRef<OsStr>) -> PathBuf {
    if cfg!(windows) {
        find(name.as_ref(), std::env::var_os("PATH").as_deref(), &["exe", "cmd", "bat"])
    } else {
        PathBuf::from(name.as_ref())
    }
}

fn find(name: &OsStr, search: Option<&OsStr>, extensions: &[&str]) -> PathBuf {
    let bare = Path::new(name);

    if bare.components().count() != 1 || bare.extension().is_some() {
        return bare.to_path_buf();
    }

    for directory in search.map(std::env::split_paths).into_iter().flatten() {
        for extension in extensions {
            let mut file = name.to_os_string();

            file.push(".");
            file.push(extension);

            let candidate = directory.join(file);

            if candidate.is_file() {
                return candidate;
            }
        }
    }

    bare.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::find;

    #[test]
    fn finds_command_shims_on_the_search_path() {
        let root = std::env::temp_dir().join(format!("krabb-program-{}", std::process::id()));
        let empty = root.join("empty");
        let shims = root.join("shims");

        std::fs::create_dir_all(&empty).unwrap();
        std::fs::create_dir_all(&shims).unwrap();
        std::fs::write(shims.join("npx.cmd"), "").unwrap();

        let search = std::env::join_paths([&empty, &shims]).unwrap();
        let extensions = ["exe", "cmd", "bat"];

        assert_eq!(find("npx".as_ref(), Some(&search), &extensions), shims.join("npx.cmd"));

        assert_eq!(
            find("npx.exe".as_ref(), Some(&search), &extensions),
            std::path::Path::new("npx.exe")
        );

        assert_eq!(find("node".as_ref(), Some(&search), &extensions), std::path::Path::new("node"));

        assert_eq!(
            find("bin/npx".as_ref(), Some(&search), &extensions),
            std::path::Path::new("bin/npx")
        );

        std::fs::remove_dir_all(root).unwrap();
    }
}
