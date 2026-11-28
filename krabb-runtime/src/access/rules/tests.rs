use super::*;

use serde_json::json;

fn rule(scope: &str, tool: &str, args: serde_json::Value) -> Rule {
    Rule::new(scope, tool, &args, Role::Owner).unwrap()
}

#[test]
fn rules_match_the_exact_command_or_path_in_their_own_conversation() {
    let test = rule("telegram:42", "shell_run", json!({"command": "cargo test"}));

    assert!(test.allows("telegram:42", "shell_run", &json!({"command": "cargo test"})));
    assert!(!test.allows("telegram:42", "shell_run", &json!({"command": "cargo test; rm -rf ~"})));
    assert!(!test.allows("telegram:7", "shell_run", &json!({"command": "cargo test"})));
    assert!(!test.allows("telegram:42", "file_write", &json!({"command": "cargo test"})));

    let file = rule("tui", "file_write", json!({"path": "notes/today.md", "content": "x"}));

    assert!(file.allows("tui", "file_write", &json!({"path": "notes/today.md"})));
    assert!(!file.allows("tui", "file_write", &json!({"path": "notes/other.md"})));

    let folder = Rule { path: Some("notes/**".into()), ..file.clone() };

    assert!(folder.allows("tui", "file_write", &json!({"path": "notes/a/b.md"})));
    assert!(!folder.allows("tui", "file_write", &json!({"path": "notes/../secret"})));
    assert!(!folder.allows("tui", "file_write", &json!({"path": "notesx/a.md"})));

    assert!(Rule::new("tui", "message_send", &json!({}), Role::Owner).is_none());
    assert!(Rule::new("tui", "shell_run", &json!({"command": ""}), Role::Owner).is_none());
    assert_eq!(test.id, rule("telegram:42", "shell_run", json!({"command": "cargo test"})).id);
    assert_ne!(test.id, file.id);
}

#[test]
fn scopes_name_the_client_or_the_channel_chat() {
    use crate::sharing::Principal;

    assert_eq!(scope(&Principal::Client("tui".into()), "session"), "tui");
    assert_eq!(scope(&Principal::Channel("telegram".into()), "42"), "telegram:42");
}

#[test]
fn admin_rules_need_a_role_whose_level_covers_them() {
    let config: Config =
        toml::from_str("preset = 'standard'\n[roles]\nadmin = { elevation = 'packages' }\n")
            .unwrap();
    let install = json!({"command": "sudo apt install htop"});
    let restart = json!({"command": "sudo systemctl restart nginx"});

    assert!(may_save(&config, Role::Owner, "shell_run", &install));
    assert!(may_save(&config, Role::Admin, "shell_run", &install));
    assert!(!may_save(&config, Role::Admin, "shell_run", &restart));
    assert!(!may_save(&config, Role::Member, "shell_run", &install));
    assert!(may_save(&config, Role::Member, "shell_run", &json!({"command": "cargo test"})));

    let mut saved = config.clone();

    saved.rules.push(Rule::new("tui", "shell_run", &install, Role::Admin).unwrap());

    assert!(allowed(&saved, "tui", "shell_run", &install));

    saved.roles.clear();

    assert!(!allowed(&saved, "tui", "shell_run", &install));
}

#[test]
fn rules_are_saved_once_listed_and_forgotten() {
    let root = crate::tests::test_root("approval-rules");

    crate::home::init_at(&root).unwrap();

    let test = rule("tui", "shell_run", json!({"command": "cargo test"}));

    save(&root, &test).unwrap();
    save(&root, &test).unwrap();

    let config = crate::load_config_at(&root).unwrap();

    assert_eq!(config.rules, [test.clone()]);
    assert!(
        listing(&config.rules).contains(&format!("{}  tui  shell_run  \"cargo test\"", test.id))
    );
    assert!(forget(&root, &test.id).unwrap());
    assert!(!forget(&root, &test.id).unwrap());
    assert!(crate::load_config_at(&root).unwrap().rules.is_empty());
    assert!(!std::fs::read_to_string(root.join("config.toml")).unwrap().contains("[[rules]]"));

    let _ = std::fs::remove_dir_all(root);
}
