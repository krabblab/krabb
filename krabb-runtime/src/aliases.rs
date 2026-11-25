//! Maps tool names from earlier releases to their current names, so saved conversations,
//! approvals, and transcripts that called them still run.

use std::borrow::Cow;

use serde_json::Value;

/// The earlier names of the task control tool, which took the action as an argument.
pub(crate) const CONTROLS: [&str; 2] = ["job", "tasks"];

/// Earlier names of host tools; plugins may not register these, so they always map.
pub(crate) const HOST: [(&str, &str); 5] = [
    ("task", crate::routing::NAME),
    ("checkpoint", crate::tasks::CHECKPOINT),
    ("input", crate::tasks::input::NAME),
    ("conversation", crate::slash::TOOL),
    ("settings", crate::settings::TOOL),
];

/// Earlier names of bundled plugin tools; a plugin that registers the old name keeps it.
const PLUGIN: [(&str, &str); 9] = [
    ("read", "file_read"),
    ("write", "file_write"),
    ("patch", "file_patch"),
    ("list", "file_list"),
    ("search", "file_search"),
    ("fetch", "web_fetch"),
    ("git", "git_run"),
    ("shell", "shell_run"),
    ("data", "data_query"),
];

/// Runner operations were offered as `operation_runner_<name>` before they became
/// `runner_<name>`.
const OPERATION: &str = "operation_";

/// The current name for a call to `name`. `owned` says whether a plugin registers that exact
/// name, which then keeps it, so a third-party `read` tool is never taken over.
pub(crate) fn name<'a>(name: &'a str, args: &Value, owned: impl Fn(&str) -> bool) -> Cow<'a, str> {
    if CONTROLS.contains(&name)
        && let Some(action) = args["action"].as_str()
        && crate::tasks::ACTIONS.contains(&action)
    {
        return Cow::Owned(format!("task_{action}"));
    }

    if let Some((_, current)) = HOST.iter().find(|(old, _)| *old == name) {
        return Cow::Borrowed(current);
    }

    if owned(name) {
        return Cow::Borrowed(name);
    }

    if let Some((_, current)) = PLUGIN.iter().find(|(old, _)| *old == name) {
        return Cow::Borrowed(current);
    }

    match name.strip_prefix(OPERATION) {
        Some(operation) if operation.starts_with(crate::operations::RUNNER) => {
            Cow::Borrowed(operation)
        }

        _ => Cow::Borrowed(name),
    }
}

/// The current name and arguments for a call; task controls drop the `action` the name now
/// carries.
pub(crate) fn current(
    name: &str,
    mut args: Value,
    owned: impl Fn(&str) -> bool,
) -> (String, Value) {
    let current = self::name(name, &args, owned).into_owned();

    if CONTROLS.contains(&name)
        && crate::tasks::action(&current).is_some()
        && let Value::Object(values) = &mut args
    {
        values.remove("action");
    }

    (current, args)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn every_earlier_name_maps_to_its_current_name_and_arguments() {
        for (action, tool) in crate::tasks::ACTIONS.into_iter().zip(crate::tasks::CONTROLS) {
            for old in CONTROLS {
                let args = json!({"action":action, "id":"task-1", "instruction":"Use v2."});

                assert_eq!(
                    current(old, args, |_| false),
                    (tool.to_owned(), json!({"id":"task-1", "instruction":"Use v2."}))
                );
            }
        }

        let renamed = [
            ("task", "task_classify"),
            ("checkpoint", "task_checkpoint"),
            ("input", "task_read"),
            ("conversation", "conversation_command"),
            ("settings", "settings_change"),
            ("read", "file_read"),
            ("write", "file_write"),
            ("patch", "file_patch"),
            ("list", "file_list"),
            ("search", "file_search"),
            ("fetch", "web_fetch"),
            ("git", "git_run"),
            ("shell", "shell_run"),
            ("data", "data_query"),
            ("operation_runner_delegate", "runner_delegate"),
        ];

        for (old, new) in renamed {
            let args = json!({"path":"note.txt", "kind":"background"});

            assert_eq!(current(old, args.clone(), |_| false), (new.to_owned(), args));
        }
    }

    #[test]
    fn current_names_and_unknown_actions_stay_as_they_are() {
        for name in ["file_read", "task_pause", "memory_search", "browser_open", "operation_x"] {
            assert_eq!(name, self::name(name, &json!({}), |_| false));
        }

        let approve = json!({"action":"approve", "id":"task-1"});

        assert_eq!(current("job", approve.clone(), |_| false), ("job".to_owned(), approve));
    }

    #[test]
    fn a_plugin_that_owns_an_earlier_name_keeps_it() {
        let owned = |name: &str| matches!(name, "read" | "data" | "operation_runner_delegate");

        assert_eq!(self::name("read", &json!({}), owned), "read");
        assert_eq!(self::name("data", &json!({}), owned), "data");
        assert_eq!(
            self::name("operation_runner_delegate", &json!({}), owned),
            "operation_runner_delegate"
        );

        assert_eq!(self::name("write", &json!({}), owned), "file_write");
        assert_eq!(self::name("task", &json!({}), |_| true), "task_classify");
        assert_eq!(self::name("job", &json!({"action":"pause"}), |_| true), "task_pause");
    }
}
