use std::fs;

use super::{checksum, source_from, stable, target, valid_version};

const DIGEST: &str = "ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12";

#[test]
fn versions_follow_the_installer_format() {
    for version in ["0.1.0", "10.20.30", "0.2.0-rc.1", "1.0.0.beta"] {
        assert!(valid_version(version), "{version}");
    }

    for version in ["", "0.1", "v0.1.0", "0.1.x", "0.1.0-", "0..1", "0.1.0+build", "main"] {
        assert!(!valid_version(version), "{version}");
    }
}

#[test]
fn targets_cover_the_published_matrix() {
    assert_eq!(target("linux", "x86_64"), Some(("x86_64-unknown-linux-gnu", "tar.gz")));
    assert_eq!(target("macos", "aarch64"), Some(("aarch64-apple-darwin", "tar.gz")));
    assert_eq!(target("windows", "aarch64"), Some(("aarch64-pc-windows-msvc", "zip")));
    assert_eq!(target("freebsd", "x86_64"), None);
    assert_eq!(target("linux", "riscv64"), None);
}

#[test]
fn checksums_match_exact_archive_names() {
    let sums = format!(
        "{DIGEST}  krabb-telegram-v0.1.0-x86_64-unknown-linux-gnu.tar.gz.sig\n\
         {}  *krabb-telegram-v0.1.0-x86_64-unknown-linux-gnu.tar.gz\n\
         short  krabb-tui-v0.1.0-x86_64-unknown-linux-gnu.tar.gz\n",
        DIGEST.to_ascii_uppercase()
    );

    assert_eq!(
        checksum(&sums, "krabb-telegram-v0.1.0-x86_64-unknown-linux-gnu.tar.gz").as_deref(),
        Some(DIGEST)
    );

    assert_eq!(checksum(&sums, "krabb-tui-v0.1.0-x86_64-unknown-linux-gnu.tar.gz"), None);
    assert_eq!(checksum(&sums, "krabb-slack-v0.1.0-x86_64-unknown-linux-gnu.tar.gz"), None);
}

#[test]
fn sources_resolve_from_a_release_directory() {
    let root = super::super::temp("krabb-release-test").unwrap();

    fs::write(
        root.join("SHA256SUMS"),
        format!("{DIGEST}  krabb-core-v0.1.0-aarch64-apple-darwin.tar.gz\n"),
    )
    .unwrap();

    let base = format!("file://{}/", root.display());

    assert_eq!(
        source_from("0.1.0", Some(&base), "macos", "aarch64").unwrap(),
        format!(
            "file://{}/krabb-core-v0.1.0-aarch64-apple-darwin.tar.gz#sha256={DIGEST}",
            root.display()
        )
    );

    let missing = source_from("0.1.0", Some(&base), "macos", "x86_64").unwrap_err();

    assert!(missing.to_string().contains("has no Krabb build for x86_64-apple-darwin"));

    let unsupported = source_from("0.1.0", Some(&base), "freebsd", "x86_64").unwrap_err();

    assert!(unsupported.to_string().contains("not published for freebsd"));

    let unavailable =
        source_from("0.1.0", Some("file:///missing/krabb"), "macos", "aarch64").unwrap_err();

    assert!(unavailable.to_string().starts_with("Release v0.1.0 is unavailable:"));

    let insecure = source_from("0.1.0", Some("http://example.com"), "macos", "aarch64");

    assert!(insecure.unwrap_err().to_string().contains("https://"));
    assert!(source_from("main", None, "macos", "aarch64").is_err());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn versions_order_releases_after_their_pre_releases() {
    use std::cmp::Ordering::{Equal, Greater, Less};

    assert_eq!(super::compare("0.2.0", "0.1.9"), Greater);
    assert_eq!(super::compare("0.10.0", "0.9.0"), Greater);
    assert_eq!(super::compare("1.0.0-rc.1", "1.0.0"), Less);
    assert_eq!(super::compare("1.0.0-rc.2", "1.0.0-rc.1"), Greater);
    assert_eq!(super::compare("0.1.0", "0.1.0"), Equal);
    assert_eq!(super::compare("0.2.0-rc.10", "0.2.0-rc.9"), Greater);
    assert_eq!(super::compare("0.2.0-rc.1", "0.2.0-rc.1.1"), Less);
    assert_eq!(super::compare("0.2.0-beta", "0.2.0-1"), Greater);
    assert_eq!(super::compare("0.2.0-beta", "0.2.0-alpha"), Greater);
}

#[test]
fn published_targets_match_the_release_matrix() {
    assert!(super::published("x86_64-unknown-linux-gnu"));
    assert!(super::published("aarch64-pc-windows-msvc"));
    assert!(!super::published("riscv64gc-unknown-linux-gnu"));
    assert!(!super::published(""));
    assert_eq!(super::parts(&format!("https://x/a.tar.gz#sha256={DIGEST}")).unwrap().1, DIGEST);
}

#[test]
fn latest_versions_come_from_the_host_archives() {
    let root = super::super::temp("krabb-release-latest").unwrap();

    fs::write(
        root.join("SHA256SUMS"),
        format!(
            "{DIGEST}  krabb-core-v0.2.0-x86_64-unknown-linux-gnu.tar.gz\n\
             {DIGEST}  krabb-core-v0.10.0-x86_64-unknown-linux-gnu.tar.gz\n\
             {DIGEST}  krabb-core-v0.11.0-aarch64-apple-darwin.tar.gz\n\
             {DIGEST}  krabb-tui-v0.12.0-x86_64-unknown-linux-gnu.tar.gz\n"
        ),
    )
    .unwrap();

    let base = format!("file://{}", root.display());

    assert_eq!(super::latest_from(Some(&base), "linux", "x86_64").unwrap(), "0.10.0");

    let missing = super::latest_from(Some(&base), "windows", "x86_64").unwrap_err();

    assert!(missing.to_string().contains("no Krabb build for x86_64-pc-windows-msvc"));
    assert!(super::latest_from(Some("ftp://example.com"), "linux", "x86_64").is_err());
    assert!(super::latest_from(Some(&base), "plan9", "x86_64").is_err());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn only_plain_versions_are_stable() {
    assert!(stable("0.3.0"));
    assert!(!stable("0.3.0-rc.1"));
    assert!(!stable("0.3.0.rc1"));
    assert!(!stable("0.3.0.1"));
}

#[test]
fn file_release_bases_name_a_local_folder_on_every_system() {
    assert_eq!(super::local("file:///srv/releases"), Some(PathBuf::from("/srv/releases")));
    assert_eq!(super::local("https://example.com/releases"), None);

    let drive = super::local("file:///D:/releases").unwrap();

    if cfg!(windows) {
        assert_eq!(drive, PathBuf::from("D:/releases"));
    } else {
        assert_eq!(drive, PathBuf::from("/D:/releases"));
    }

    assert_eq!(super::local("file://D:/releases"), Some(PathBuf::from("D:/releases")));
}
