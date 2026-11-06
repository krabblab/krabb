use super::*;

fn fixture() -> (Config, state::Store) {
    let config: Config = toml::from_str(
        r#"
        tools = true
        approval = "prompt"
        [channels.telegram]
        tools = false
        approval = "prompt"
        [channels.discord]
        worktree = true
        [clients.tui]
        tools = true
        visibility = [{ data = ["context"], resources = ["channels.telegram"] }]
    "#,
    )
    .unwrap();

    let mut store = state::Store::default();

    for (id, channel, text) in [
        ("tui-main", "tui", "tui context"),
        ("telegram-1", "telegram", "telegram context"),
        ("discord-2", "discord", "private discord"),
        ("old-3", "old", "unconfigured"),
    ] {
        store.ensure(id, "test").unwrap();
        store.route(id, channel, id, None, true).unwrap();
        store
            .push(
                id,
                Message {
                    id: "1".into(),
                    session: id.into(),
                    role: Role::User,
                    sender: Some(channel.into()),
                    content: vec![Content::Text { text: text.into() }],
                },
            )
            .unwrap();
    }

    (config, store)
}

#[test]
fn typed_scopes_validate_sources_and_preserve_defaults() {
    let (config, _) = fixture();

    config.validate().unwrap();

    assert!(config.tools);
    assert!(!config.channels["telegram"].tools);
    assert!(config.clients["tui"].visibility.is_some());
    assert!(!config.clients["tui"].worktree);

    for value in [
        r#"{mode="shared",sources=[]}"#,
        r#"{mode="shared",sources=["agents.a"]}"#,
        r#"{mode="shared",sources=["clients.tui","clients.tui"]}"#,
        r#"{mode="global",sources=["clients.tui"]}"#,
    ] {
        let parsed = toml::from_str::<Config>(&format!("[channels.telegram]\ncontext={value}"));

        assert!(parsed.is_err() || parsed.unwrap().validate().is_err());
    }

    assert_eq!(
        sharing::approval(&config, &sharing::principal("telegram"), ApprovalMode::Off),
        ApprovalMode::Prompt
    );
}

#[test]
fn chat_context_is_directional_bounded_and_excludes_worktrees() {
    let (config, mut store) = fixture();
    let mut messages = Vec::new();

    context::append(
        &mut messages,
        &store,
        "telegram-1",
        &sharing::sources(&config, &sharing::principal("telegram"), sharing::Data::Context),
        &config,
    );

    let text = serde_json::to_string(&messages).unwrap();

    assert!(text.contains("tui context"));
    assert!(!text.contains("private discord"));
    assert!(!text.contains("telegram context"));
    assert!(!text.contains("unconfigured"));

    messages.clear();
    context::append(&mut messages, &store, "telegram-1", &[], &config);

    assert!(messages.is_empty());

    context::append(
        &mut messages,
        &store,
        "tui-main",
        &[sharing::principal("telegram"), sharing::principal("old")],
        &config,
    );

    let text = serde_json::to_string(&messages).unwrap();

    assert!(text.contains("telegram context"));
    assert!(!text.contains("private discord"));

    store.sessions.get_mut("tui-main").unwrap().isolated = Some(PathBuf::from("isolated"));
    messages.clear();

    context::append(
        &mut messages,
        &store,
        "tui-main",
        &[sharing::principal("telegram"), sharing::principal("old")],
        &config,
    );

    assert!(messages.is_empty());
}

#[test]
fn memory_and_data_scopes_are_independent_and_worktrees_override_them() {
    let (mut config, _) = fixture();
    let own = "provider:telegram:1";
    let params = sharing::memory_params(&config, "telegram", own, false);

    assert!(sharing::visible(&params, own));
    assert!(!sharing::visible(&params, "tui:main"));
    assert!(sharing::visible(&params, "global"));

    config.clients.get_mut("tui").unwrap().visibility = Some(vec![sharing::Rule {
        data: vec![sharing::Data::Memory],
        resources: vec![krabb_core::visibility::Resource::Global],
    }]);

    let params = sharing::memory_params(&config, "telegram", own, false);

    assert!(sharing::visible(&params, "tui:main"));
    assert!(!sharing::visible(&params, "provider:discord:2"));
    assert!(!sharing::visible(&params, "provider:old:3"));
    assert!(sharing::visible(&params, "global"));

    let params = sharing::memory_params(&config, "telegram", own, true);

    assert!(!sharing::visible(&params, "tui:main"));
    assert!(!sharing::visible(&params, "global"));

    assert!(!sharing::visible(
        &sharing::params(&config, "telegram", own, false, sharing::Data::Records),
        "tui:main"
    ));
}

#[test]
fn plain_files_keep_one_workspace_without_hidden_state() {
    let root = tests::test_root("visibility-workspace");

    std::fs::create_dir_all(&root).unwrap();

    assert_eq!(sharing::workspace(&root).unwrap(), Some(std::fs::canonicalize(&root).unwrap()));
    assert!(!root.join(".krabb").exists());

    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn worktree_tools_cannot_escape_through_unconfined_capabilities() {
    let plugins = Plugins::default();
    let sessions = Arc::new(Mutex::new(state::Store::default()));
    let approvals = Arc::new(AsyncMutex::new(approval::Gate::new().unwrap()));
    let workspace = PathBuf::from("/workspace/.krabb/worktrees/test");

    for name in ["shell", "git", "patch", "fetch", "computer", "schedule_propose"] {
        let (notices, _receiver) = tokio::sync::mpsc::channel(1);

        let result = execute_tool(
            &plugins,
            1,
            name,
            serde_json::json!({}),
            "fixture",
            "fixture",
            ApprovalMode::Auto,
            Arc::clone(&approvals),
            "telegram",
            "1",
            None,
            "test",
            &sessions,
            Some(&workspace),
            true,
            true,
            true,
            notices,
            tokio::time::Instant::now() + Duration::from_secs(1),
            &mut None,
            None,
            None,
            0,
        )
        .await;

        assert!(result.unwrap_err().to_string().contains("isolated workspace boundary"));
    }
}

#[test]
fn schedules_receive_their_origin_context_without_exporting_other_private_chats() {
    let (mut config, store) = fixture();

    config.clients.get_mut("tui").unwrap().visibility = Some(Vec::new());

    let origin = schedules::Origin {
        channel: "telegram".into(),
        chat: "telegram-1".into(),
        session: "telegram-1".into(),
        thread: None,
        private: Some(true),
        sender: None,
        roles: Vec::new(),
    };

    let mut messages = Vec::new();

    context::schedule(&mut messages, &store, "schedule-1", &origin, &config);

    let text = serde_json::to_string(&messages).unwrap();

    assert!(text.contains("telegram context"));
    assert!(!text.contains("tui context"));
    assert!(!text.contains("private discord"));

    config.clients.get_mut("tui").unwrap().visibility = Some(vec![sharing::Rule {
        data: vec![sharing::Data::Context],
        resources: vec![krabb_core::visibility::Resource::Principal(sharing::principal(
            "telegram",
        ))],
    }]);

    messages.clear();
    context::schedule(&mut messages, &store, "schedule-1", &origin, &config);

    let text = serde_json::to_string(&messages).unwrap();

    assert!(text.contains("telegram context"));
    assert!(text.contains("tui context"));
    assert!(!text.contains("private discord"));
}
