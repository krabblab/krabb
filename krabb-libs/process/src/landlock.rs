//! Builds Landlock rulesets that confine a command's filesystem access.

use std::{
    fs::OpenOptions,
    io,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
        unix::{fs::OpenOptionsExt, process::CommandExt},
    },
    path::Path,
    process::Command,
    sync::Arc,
};

const CREATE_VERSION: libc::c_uint = 1;
const PATH_BENEATH: libc::c_int = 1;

const EXECUTE: u64 = 1 << 0;
const WRITE_FILE: u64 = 1 << 1;
const READ_FILE: u64 = 1 << 2;
const READ_DIR: u64 = 1 << 3;
const TRUNCATE: u64 = 1 << 14;
const IOCTL_DEV: u64 = 1 << 15;

const READ: u64 = EXECUTE | READ_FILE | READ_DIR;
const FILE: u64 = EXECUTE | WRITE_FILE | READ_FILE | TRUNCATE | IOCTL_DEV;
const DEVICE: u64 = READ_FILE | WRITE_FILE | TRUNCATE | IOCTL_DEV;

const SYSTEM: [&str; 13] = [
    "/usr",
    "/bin",
    "/sbin",
    "/lib",
    "/lib32",
    "/lib64",
    "/libx32",
    "/etc",
    "/opt",
    "/nix/store",
    "/proc",
    "/sys",
    "/run/systemd/resolve",
];

const DEVICES: [&str; 7] =
    ["/dev/null", "/dev/zero", "/dev/full", "/dev/random", "/dev/urandom", "/dev/tty", "/dev/ptmx"];

#[repr(C)]
struct RulesetAttr {
    handled_access_fs: u64,
}

#[repr(C, packed)]
struct PathBeneathAttr {
    allowed_access: u64,
    parent_fd: RawFd,
}

/// The Landlock ABI version the kernel supports, or a negative value when it has none.
pub(crate) fn abi() -> i64 {
    // SAFETY: A null attribute with size zero and the version flag only queries the ABI.
    unsafe {
        libc::syscall(
            libc::SYS_landlock_create_ruleset,
            std::ptr::null::<RulesetAttr>(),
            0_usize,
            CREATE_VERSION,
        )
    }
}

/// The filesystem rights the given ABI can restrict, or none when Landlock is unavailable.
pub(crate) fn handled(abi: i64) -> Option<u64> {
    match abi {
        i64::MIN..=0 => None,
        1 => Some((1 << 13) - 1),
        2 => Some((1 << 14) - 1),
        3 | 4 => Some((1 << 15) - 1),
        _ => Some((1 << 16) - 1),
    }
}

/// A ruleset that grants ROOT and TEMP fully, the system paths read-only, and a few devices.
pub(crate) fn ruleset(root: &Path, temp: &Path, abi: i64) -> io::Result<OwnedFd> {
    let handled = handled(abi)
        .ok_or_else(|| io::Error::new(io::ErrorKind::Unsupported, "Landlock is unavailable."))?;

    let attr = RulesetAttr { handled_access_fs: handled };

    // SAFETY: attr is a valid ruleset attribute that outlives the call.
    let fd = unsafe {
        libc::syscall(
            libc::SYS_landlock_create_ruleset,
            &attr as *const RulesetAttr,
            std::mem::size_of::<RulesetAttr>(),
            0_u32,
        )
    };

    if fd < 0 {
        return Err(io::Error::last_os_error());
    }

    // SAFETY: The kernel returned a new descriptor that nothing else owns.
    let ruleset = unsafe { OwnedFd::from_raw_fd(fd as RawFd) };

    for path in [root, temp] {
        add(&ruleset, path, handled, true)?;
    }

    for path in SYSTEM {
        add(&ruleset, Path::new(path), READ & handled, false)?;
    }

    add(&ruleset, Path::new("/dev"), READ_DIR, false)?;

    for path in DEVICES {
        add(&ruleset, Path::new(path), DEVICE & handled, false)?;
    }

    Ok(ruleset)
}

fn add(ruleset: &OwnedFd, path: &Path, access: u64, required: bool) -> io::Result<()> {
    let file = match OpenOptions::new().read(true).custom_flags(libc::O_PATH).open(path) {
        Ok(file) => file,
        Err(error) if !required && error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };

    let access = if file.metadata()?.is_dir() { access } else { access & FILE };

    let attr = PathBeneathAttr { allowed_access: access, parent_fd: file.as_raw_fd() };

    // SAFETY: attr names a live descriptor and stays valid for the duration of the call.
    let result = unsafe {
        libc::syscall(
            libc::SYS_landlock_add_rule,
            ruleset.as_raw_fd(),
            PATH_BENEATH,
            &attr as *const PathBeneathAttr,
            0_u32,
        )
    };

    if result != 0 {
        return Err(io::Error::last_os_error());
    }

    Ok(())
}

/// Applies RULESET to the command after fork, so only the child and its descendants are confined.
pub(crate) fn restrict(command: &mut Command, ruleset: Arc<OwnedFd>) -> &mut Command {
    // SAFETY: The callback only makes async-signal-safe syscalls on a descriptor it owns.
    unsafe { command.pre_exec(move || restrict_in_child(ruleset.as_raw_fd())) }
}

fn restrict_in_child(ruleset: RawFd) -> io::Result<()> {
    // SAFETY: prctl with PR_SET_NO_NEW_PRIVS takes no pointers.
    if unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } != 0 {
        return Err(io::Error::last_os_error());
    }

    // SAFETY: ruleset is a live Landlock ruleset descriptor.
    if unsafe { libc::syscall(libc::SYS_landlock_restrict_self, ruleset, 0_u32) } != 0 {
        return Err(io::Error::last_os_error());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{abi, handled, ruleset};

    use std::path::Path;

    #[test]
    fn maps_each_abi_to_the_rights_it_restricts() {
        assert_eq!(handled(-1), None);
        assert_eq!(handled(0), None);
        assert_eq!(handled(1), Some(0x1fff));
        assert_eq!(handled(2), Some(0x3fff));
        assert_eq!(handled(4), Some(0x7fff));
        assert_eq!(handled(7), Some(0xffff));
    }

    #[test]
    fn refuses_a_ruleset_without_landlock() {
        let error = ruleset(Path::new("/"), Path::new("/"), -1).unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::Unsupported);
    }

    #[test]
    fn refuses_a_missing_root() {
        if handled(abi()).is_none() {
            return;
        }

        assert!(ruleset(Path::new("/krabb-missing-root"), Path::new("/"), abi()).is_err());
    }
}
