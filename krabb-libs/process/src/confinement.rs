//! Confines a command to one folder with the operating system's own sandbox.

use std::{
    ffi::{OsStr, OsString},
    io,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        OnceLock,
        atomic::{AtomicU64, Ordering},
    },
};

#[cfg(target_os = "linux")]
use std::{os::fd::OwnedFd, sync::Arc};

static TEMPS: AtomicU64 = AtomicU64::new(0);

/// The operating system sandbox that confines commands on this machine.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mechanism {
    Landlock,
    Seatbelt,
}

impl Mechanism {
    /// The sandbox this machine offers, detected once per process.
    pub fn current() -> Option<Self> {
        static CURRENT: OnceLock<Option<Mechanism>> = OnceLock::new();

        *CURRENT.get_or_init(detect)
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Landlock => "Landlock",
            Self::Seatbelt => "the macOS sandbox",
        }
    }
}

#[cfg(target_os = "linux")]
fn detect() -> Option<Mechanism> {
    crate::landlock::handled(crate::landlock::abi()).map(|_| Mechanism::Landlock)
}

#[cfg(target_os = "macos")]
fn detect() -> Option<Mechanism> {
    crate::seatbelt::available().then_some(Mechanism::Seatbelt)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn detect() -> Option<Mechanism> {
    None
}

/// Whether commands can be confined to ROOT on this machine while HOME stays out of reach.
pub fn supports(root: &Path, home: &Path) -> bool {
    Mechanism::current().is_some() && separate(root, home)
}

fn separate(root: &Path, home: &Path) -> bool {
    let Ok(root) = root.canonicalize() else {
        return false;
    };

    let home = home.canonicalize().unwrap_or_else(|_| home.to_path_buf());

    !home.starts_with(root)
}

/// A sandbox that lets commands read and write only a root folder and a private temporary
/// folder, and read the system paths needed to run programs.
#[derive(Debug)]
pub struct Confinement {
    root: PathBuf,
    temp: PathBuf,

    #[cfg(target_os = "linux")]
    ruleset: Arc<OwnedFd>,
}

impl Confinement {
    /// Prepares a sandbox for ROOT, refusing when the system has none or ROOT contains HOME.
    pub fn new(root: &Path, home: &Path) -> io::Result<Self> {
        let Some(mechanism) = Mechanism::current() else {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "This system has no OS sandbox.",
            ));
        };

        if !separate(root, home) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "The sandbox root contains Krabb's home.",
            ));
        }

        let root = root.canonicalize()?;
        let temp = temp()?;

        let confinement = Self::build(mechanism, root, temp.clone());

        if confinement.is_err() {
            let _ = std::fs::remove_dir_all(&temp);
        }

        confinement
    }

    #[cfg(target_os = "linux")]
    fn build(_: Mechanism, root: PathBuf, temp: PathBuf) -> io::Result<Self> {
        let ruleset = crate::landlock::ruleset(&root, &temp, crate::landlock::abi())?;

        Ok(Self { root, temp, ruleset: Arc::new(ruleset) })
    }

    #[cfg(not(target_os = "linux"))]
    fn build(_: Mechanism, root: PathBuf, temp: PathBuf) -> io::Result<Self> {
        Ok(Self { root, temp })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn temp(&self) -> &Path {
        &self.temp
    }

    /// A command for PROGRAM that runs inside this sandbox with its temporary folder as home.
    pub fn command(&self, program: impl AsRef<OsStr>) -> Command {
        let mut command = self.wrap(program.as_ref());

        for name in ["TMPDIR", "TMP", "TEMP", "HOME"] {
            command.env(name, &self.temp);
        }

        for name in ["XDG_CONFIG_HOME", "XDG_CACHE_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME"] {
            command.env_remove(name);
        }

        command
    }

    #[cfg(target_os = "linux")]
    fn wrap(&self, program: &OsStr) -> Command {
        let mut command = Command::new(program);

        crate::landlock::restrict(&mut command, Arc::clone(&self.ruleset));
        command
    }

    #[cfg(target_os = "macos")]
    fn wrap(&self, program: &OsStr) -> Command {
        let mut command = Command::new(crate::seatbelt::PROGRAM);

        command
            .arg("-p")
            .arg(crate::seatbelt::PROFILE)
            .arg("-D")
            .arg(define("ROOT", &self.root))
            .arg("-D")
            .arg(define("TEMP", &self.temp))
            .arg(program);

        command
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    fn wrap(&self, program: &OsStr) -> Command {
        Command::new(program)
    }
}

impl Drop for Confinement {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.temp);
    }
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn define(name: &str, path: &Path) -> OsString {
    let mut value = OsString::from(name);

    value.push("=");
    value.push(path);
    value
}

fn temp() -> io::Result<PathBuf> {
    let parent = std::env::temp_dir().canonicalize()?;

    for _ in 0..16 {
        let nonce = TEMPS.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!("krabb-sandbox-{}-{nonce}", std::process::id()));
        let mut builder = std::fs::DirBuilder::new();

        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);

        match builder.create(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }

    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "Could not allocate a private sandbox folder.",
    ))
}

#[cfg(test)]
mod tests {
    use super::{Confinement, Mechanism, define, separate, supports};

    use std::path::{Path, PathBuf};

    struct Folder(PathBuf);

    impl Folder {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("krabb-confinement-{name}-{}", std::process::id()));

            let _ = std::fs::remove_dir_all(&path);

            std::fs::create_dir_all(&path).unwrap();
            Self(path.canonicalize().unwrap())
        }
    }

    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn run(confinement: &Confinement, root: &Path, script: &str) -> std::process::Output {
        let mut command = confinement.command("sh");

        command.args(["-c", script]).current_dir(root).output().unwrap()
    }

    #[test]
    fn keeps_a_home_inside_the_root_unconfinable() {
        let root = Folder::new("nested");
        let home = root.0.join("home");

        assert!(!separate(&root.0, &home));
        assert!(!separate(&root.0.join("missing"), Path::new("/elsewhere")));
        assert!(!supports(&root.0, &root.0));
        assert!(Confinement::new(&root.0, &home).is_err());
        assert_eq!(define("ROOT", Path::new("/a b")), "ROOT=/a b");
    }

    #[test]
    fn names_each_mechanism() {
        assert_eq!(Mechanism::Landlock.name(), "Landlock");
        assert_eq!(Mechanism::Seatbelt.name(), "the macOS sandbox");
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn confines_commands_to_the_root_and_temporary_folder() {
        if cfg!(target_os = "linux") && Mechanism::current().is_none() {
            eprintln!("Landlock is unavailable; skipping the confinement test.");
            return;
        }

        let root = Folder::new("root");
        let home = Folder::new("home");
        let outside = Folder::new("outside");
        let credentials = home.0.join("credentials");

        std::fs::write(&credentials, "secret").unwrap();
        std::fs::write(outside.0.join("note"), "outside").unwrap();

        assert!(supports(&root.0, &home.0));

        let confinement = Confinement::new(&root.0, &home.0).unwrap();
        let temp = confinement.temp().to_path_buf();

        assert_eq!(confinement.root(), root.0);
        assert!(temp.is_dir());

        let inside = run(
            &confinement,
            &root.0,
            "printf inside > note && cat note && printf scratch > \"$TMPDIR/x\" && cat \"$HOME/x\" \
             && printf x > /dev/null",
        );

        assert!(inside.status.success(), "{}", String::from_utf8_lossy(&inside.stderr));
        assert_eq!(inside.stdout, b"insidescratch");

        let read = run(&confinement, &root.0, &format!("cat '{}'", credentials.display()));

        assert!(!read.status.success());
        assert!(!String::from_utf8_lossy(&read.stdout).contains("secret"));

        let listed = run(&confinement, &root.0, &format!("ls '{}'", outside.0.display()));

        assert!(!listed.status.success());

        let written =
            run(&confinement, &root.0, &format!("printf x > '{}/new'", outside.0.display()));

        assert!(!written.status.success());
        assert!(!outside.0.join("new").exists());

        let moved = run(&confinement, &root.0, &format!("mv note '{}/moved'", outside.0.display()));

        assert!(!moved.status.success());
        assert!(root.0.join("note").exists());

        drop(confinement);

        assert!(!temp.exists());
    }
}
