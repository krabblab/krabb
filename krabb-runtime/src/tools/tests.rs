use super::{
    ToolContent, ToolRegistry, ToolResult, data_tools, decode_tool_result,
    schedule_listing_allowed, scheduler_tools, tool_result_value, tools, validate_registered_args,
    validate_tool_spec,
};

#[test]
fn shell_is_only_declared_when_enabled() {
    assert!(!tools(false).iter().any(|tool| tool.name == "shell"));

    let shell = tools(true).into_iter().find(|tool| tool.name == "shell").unwrap();

    assert!(shell.description.unwrap().contains("paths beyond the workspace"));
}

#[test]
fn fetch_tool_requires_a_url() {
    let fetch = tools(false).into_iter().find(|tool| tool.name == "fetch").unwrap();

    assert_eq!(fetch.schema["required"], serde_json::json!(["url"]));
    assert_eq!(fetch.schema["properties"]["url"]["type"], "string");
}

#[test]
fn structured_data_tool_has_a_bounded_schema_and_guidance() {
    let data = data_tools().pop().unwrap();

    validate_tool_spec(&data).unwrap();

    assert_eq!(data.name, "data");
    assert!(data.description.unwrap().contains("structured data"));
    assert_eq!(data.schema["properties"]["columns"]["maxItems"], 64);
    assert!(data.schema["properties"]["columns"].get("minItems").is_none());
}

#[test]
fn validates_and_preserves_multimodal_tool_results() {
    let result = decode_tool_result(serde_json::json!({
        "content": [
            {"kind":"text", "text":"Screen captured."},
            {"kind":"image", "uri":"data:image/png;base64,iVBORw0KGgo=", "alt":"desktop"}
        ],
        "metadata": {"width": 1280, "height": 720}
    }))
    .unwrap();

    assert_eq!(result.content.len(), 2);

    assert!(
        matches!(&result.content[1], ToolContent::Image { uri, .. } if uri.starts_with("data:image/png"))
    );

    assert_eq!(result.metadata.as_ref().unwrap()["width"], 1280);

    let wire = tool_result_value(result).unwrap();

    assert_eq!(wire["content"][1]["kind"], "image");

    assert_eq!(
        wire["text"],
        "Screen captured.\ndesktop\nTool metadata: {\"height\":720,\"width\":1280}"
    );
}

#[test]
fn rejects_unbounded_or_untrusted_tool_images_and_schemas() {
    assert!(
        decode_tool_result(serde_json::json!({
            "content": [{"kind":"image", "uri":"https://example.test/image.png", "alt":null}]
        }))
        .is_err()
    );

    assert!(
        decode_tool_result(serde_json::json!({
            "content": [{"kind":"image", "uri":"data:image/png;base64,not-an-image", "alt":null}]
        }))
        .is_err()
    );

    let bad = super::ToolSpec {
        name: "bad name".into(),
        description: None,
        schema: serde_json::json!({"type":"object", "properties":{}}),
    };

    assert!(validate_tool_spec(&bad).is_err());

    let result = ToolResult::text("plain result");

    assert_eq!(tool_result_value(result).unwrap()["text"], "plain result");
}

#[test]
fn validates_registered_arguments_before_execution() {
    let specs = tools(false);

    let registry = ToolRegistry {
        owners: specs.iter().map(|spec| (spec.name.clone(), "tools".into())).collect(),
        specs,
        ..ToolRegistry::default()
    };

    assert!(validate_registered_args(&registry, "read", &serde_json::json!({"path":"a"})).is_ok());

    assert!(validate_registered_args(&registry, "read", &serde_json::json!({})).is_err());

    assert!(
        validate_registered_args(
            &registry,
            "read",
            &serde_json::json!({"path":"a", "outside":true})
        )
        .is_err()
    );
}

#[test]
fn scheduler_describes_available_execution_capabilities() {
    let channel_ids = vec!["discord".to_owned(), "telegram".to_owned()];

    let proposal = scheduler_tools(true, false, false, &channel_ids)
        .into_iter()
        .find(|tool| tool.name == "schedule_propose")
        .unwrap();

    let description = proposal.description.unwrap();

    assert!(description.contains("workspace tools are enabled"));
    assert!(description.contains("local shell is disabled"));
    assert!(description.contains("external channels are discord, telegram"));
    assert!(description.contains("local TUI session messages are available"));
    assert!(description.contains("unique funny name"));
    assert!(proposal.schema["properties"].get("name").is_none());
    assert_eq!(proposal.schema["properties"]["repeat_seconds"]["minimum"], 5);

    let unavailable = scheduler_tools(false, true, true, &[])
        .into_iter()
        .find(|tool| tool.name == "schedule_propose")
        .unwrap();

    let description = unavailable.description.unwrap();

    assert!(description.contains("workspace tools are disabled"));
    assert!(description.contains("local shell is disabled"));
    assert!(description.contains("external channels are none"));

    assert!(
        !scheduler_tools(true, false, false, &channel_ids)
            .iter()
            .any(|tool| tool.name == "schedule_list")
    );

    assert!(
        scheduler_tools(true, false, true, &channel_ids)
            .iter()
            .any(|tool| tool.name == "schedule_list")
    );

    assert!(schedule_listing_allowed(crate::state::SessionKind::Conversation));
    assert!(!schedule_listing_allowed(crate::state::SessionKind::Schedule));
    assert!(!schedule_listing_allowed(crate::state::SessionKind::Task));
}
