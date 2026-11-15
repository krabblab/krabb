/// Built-in client identities shared by the host and client integrations.
pub const CLI: &str = "cli";
pub const TUI: &str = "tui";
pub const BUILTIN: &[&str] = &[CLI, TUI];

pub fn known(id: &str) -> bool {
    BUILTIN.contains(&id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_identities_do_not_include_channels() {
        assert!(known(CLI));
        assert!(known(TUI));
        assert!(!known("telegram"));
    }
}
