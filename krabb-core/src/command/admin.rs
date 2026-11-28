//! Whether a parsed command changes the system in a way that usually needs admin rights.

use super::parse::Segment;

const PROGRAMS: &[&str] = &[
    "mount",
    "umount",
    "useradd",
    "userdel",
    "usermod",
    "groupadd",
    "passwd",
    "chpasswd",
    "visudo",
    "modprobe",
    "insmod",
    "rmmod",
    "reboot",
    "shutdown",
    "poweroff",
    "halt",
    "netsh",
    "bcdedit",
    "diskpart",
    "dism",
    "sfc",
    "takeown",
    "set-executionpolicy",
    "new-service",
    "set-service",
    "start-service",
    "stop-service",
    "restart-service",
];

const SERVICE_ACTIONS: &[&str] = &[
    "start",
    "stop",
    "restart",
    "reload",
    "enable",
    "disable",
    "mask",
    "unmask",
    "daemon-reload",
    "kill",
    "edit",
    "set-property",
];

const WINDOWS_QUERIES: &[&str] = &["query", "queryex", "qc", "view", "use", "help"];

/// Whether SEGMENT's program changes system services, users, devices, or settings.
pub fn administers(segment: &Segment) -> bool {
    let name = segment.program.as_str();
    let operands = segment.operands();
    let action = operands.first().map(|action| action.to_ascii_lowercase()).unwrap_or_default();

    match name {
        "systemctl" => {
            !segment.args.iter().any(|arg| arg == "--user")
                && SERVICE_ACTIONS.contains(&action.as_str())
        }

        "service" => operands.len() > 1 && operands[1] != "status",
        "reg" | "sc" | "net" => !action.is_empty() && !WINDOWS_QUERIES.contains(&action.as_str()),
        name => PROGRAMS.contains(&name),
    }
}
