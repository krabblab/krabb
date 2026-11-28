//! Runs commands under the built-in macOS sandbox with a generated profile.

use std::process::{Command, Stdio};

pub(crate) const PROGRAM: &str = "/usr/bin/sandbox-exec";

/// Denies everything except running programs, the network, reading the system's runtime paths,
/// and reading and writing the ROOT and TEMP parameters.
pub(crate) const PROFILE: &str = r#"(version 1)
(deny default)
(allow process-exec process-fork process-info* pseudo-tty sysctl-read user-preference-read)
(allow signal (target same-sandbox))
(allow network*)
(allow mach-lookup
    (global-name "com.apple.system.opendirectoryd.libinfo")
    (global-name "com.apple.system.DirectoryService.libinfo_v1")
    (global-name "com.apple.system.logger")
    (global-name "com.apple.system.notification_center")
    (global-name "com.apple.logd")
    (global-name "com.apple.dnssd.service")
    (global-name "com.apple.SystemConfiguration.configd")
    (global-name "com.apple.SystemConfiguration.DNSConfiguration")
    (global-name "com.apple.trustd")
    (global-name "com.apple.trustd.agent")
    (global-name "com.apple.cfprefsd.daemon")
    (global-name "com.apple.cfprefsd.agent"))
(allow ipc-posix-shm-read-data ipc-posix-shm-read-metadata
    (ipc-posix-name "apple.shm.notification_center"))
(allow file-read-metadata)
(allow file-read*
    (literal "/")
    (subpath "/usr")
    (subpath "/bin")
    (subpath "/sbin")
    (subpath "/System")
    (subpath "/Library")
    (subpath "/Applications")
    (subpath "/opt")
    (subpath "/private/etc")
    (subpath "/private/var/db/dyld")
    (subpath "/private/var/db/timezone")
    (subpath "/private/var/select")
    (literal "/private/var/db/xcode_select_link")
    (literal "/dev")
    (literal "/dev/random")
    (literal "/dev/urandom"))
(allow file-read* file-write* file-ioctl
    (literal "/dev/null")
    (literal "/dev/zero")
    (literal "/dev/tty")
    (literal "/dev/dtracehelper")
    (subpath "/dev/fd")
    (regex #"^/dev/ttys[0-9]+$"))
(allow file-read* file-write* (subpath (param "ROOT")) (subpath (param "TEMP")))
"#;

/// Whether sandbox-exec can apply a profile here; it cannot inside another sandbox.
pub(crate) fn available() -> bool {
    Command::new(PROGRAM)
        .args(["-p", "(version 1)(allow default)", "/usr/bin/true"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(test)]
mod tests {
    use super::{PROFILE, available};

    #[test]
    fn applies_profiles_on_this_mac() {
        assert!(available());
        assert!(PROFILE.starts_with("(version 1)\n(deny default)"));
        assert!(PROFILE.contains("(param \"ROOT\")"));
    }
}
