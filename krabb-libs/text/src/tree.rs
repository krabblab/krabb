//! The tree layout that plugin results, the CLI, and the TUI share.

/// One labeled branch with its details nested beneath it.
pub fn format_tree_entry(label: &str, details: &[String], is_last: bool) -> String {
    let root_branch = if is_last { "└─" } else { "├─" };

    let child_prefix = if is_last { "   " } else { "│  " };

    let mut lines = vec![format!("{root_branch} {label}")];

    lines.extend(details.iter().enumerate().map(|(index, detail)| {
        let branch = if index + 1 == details.len() { "└─" } else { "├─" };

        format!("{child_prefix}{branch} {detail}")
    }));

    lines.join("\n")
}

/// One tree of labels without details.
pub fn format_tree_list(entries: &[String]) -> String {
    let entries = entries.iter().map(|entry| (entry.clone(), Vec::new())).collect::<Vec<_>>();

    format_tree(&entries)
}

/// One tree of labeled entries, each with its own details, as plugin results print them.
pub fn format_tree(entries: &[(String, Vec<String>)]) -> String {
    entries
        .iter()
        .enumerate()
        .map(|(index, (label, details))| {
            format_tree_entry(label, details, index + 1 == entries.len())
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nests_details_under_each_branch() {
        let tree = format_tree(&[
            ("Runtime: running".into(), Vec::new()),
            ("Plugins: 2 installed".into(), vec!["codex: ready".into(), "echo: ready".into()]),
        ]);

        assert_eq!(
            tree,
            "├─ Runtime: running\n└─ Plugins: 2 installed\n   ├─ codex: ready\n   └─ echo: ready"
        );
    }

    #[test]
    fn keeps_the_trunk_beside_details_of_inner_branches() {
        let entry = format_tree_entry("codex", &["ready".into()], false);

        assert_eq!(entry, "├─ codex\n│  └─ ready");
        assert_eq!(format_tree_list(&["one".into(), "two".into()]), "├─ one\n└─ two");
        assert_eq!(format_tree(&[]), "");
    }
}
