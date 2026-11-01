use crate::*;
use serde_json::{Value, json};

fn invoke(
    db: &Arc<Mutex<Connection>>,
    scope: &str,
    operation: &str,
    extra: Value,
) -> krabb_core::Result<Value> {
    let mut params = json!({
        "scope": scope, "operation": operation, "database": "contacts", "table": "person"
    });

    params.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
    Ok(store(db, Request::call(1, "data", params))?.unwrap().result.unwrap())
}

fn populate(db: &Arc<Mutex<Connection>>, scope: &str, person: &str) -> Value {
    invoke(
        db,
        scope,
        "create_table",
        json!({"columns": [
            {"name": "name", "type": "text", "required": true},
            {"name": "age", "type": "integer"}
        ]}),
    )
    .unwrap();

    invoke(db, scope, "insert", json!({"record": {"name": person, "age": 36}})).unwrap()
}

#[test]
fn colliding_namespaces_reject_foreign_record_ids() {
    let db = tests::db();
    let one = populate(&db, "tui:one", "Ada");
    let two = populate(&db, "tui:two", "Grace");

    for (scope, own, foreign) in [("tui:one", &one, &two), ("tui:two", &two, &one)] {
        assert_eq!(invoke(&db, scope, "query", json!({})).unwrap()["items"], json!([own]));

        let error =
            invoke(&db, scope, "update", json!({"id": foreign["id"], "record": {"age": 99}}))
                .unwrap_err();

        assert!(error.to_string().contains("data record was not found"), "{error}");

        assert_eq!(
            invoke(&db, scope, "delete", json!({"id": foreign["id"]})).unwrap(),
            json!({"deleted": false})
        );
    }

    assert_eq!(invoke(&db, "tui:two", "query", json!({})).unwrap()["items"], json!([two]));

    assert_eq!(
        invoke(&db, "tui:one", "update", json!({"id": one["id"], "record": {"age": 37}})).unwrap()
            ["record"],
        json!({"name": "Ada", "age": 37})
    );

    assert_eq!(
        invoke(&db, "tui:one", "delete", json!({"id": one["id"]})).unwrap(),
        json!({"deleted": true})
    );

    assert_eq!(invoke(&db, "tui:one", "query", json!({})).unwrap()["items"], json!([]));
    assert_eq!(invoke(&db, "tui:two", "query", json!({})).unwrap()["items"], json!([two]));
}

#[test]
fn namespace_mutations_preserve_the_competing_scope() {
    for operation in ["drop_column", "drop_table", "drop_database"] {
        let db = tests::db();
        let one = populate(&db, "tui:one", "Ada");
        let two = populate(&db, "tui:two", "Grace");
        let schema = invoke(&db, "tui:two", "describe_table", json!({})).unwrap();
        let result = invoke(&db, "tui:one", operation, json!({"column_name": "age"})).unwrap();

        if operation == "drop_column" {
            assert_eq!(result["records_updated"], 1);

            assert_eq!(
                invoke(&db, "tui:one", "query", json!({})).unwrap()["items"],
                json!([{"id": one["id"], "record": {"name": "Ada"}}])
            );

            assert_eq!(
                invoke(&db, "tui:one", "describe_table", json!({})).unwrap()["columns"],
                json!([{"name": "name", "type": "text", "required": true}])
            );
        } else {
            assert_eq!(result, json!({"deleted": true}));

            assert_eq!(
                invoke(&db, "tui:one", "list_tables", json!({})).unwrap()["tables"],
                json!([])
            );

            let remaining: i64 = db
                .lock()
                .unwrap()
                .query_row("SELECT COUNT(*) FROM data_record WHERE scope = 'tui:one'", [], |row| {
                    row.get(0)
                })
                .unwrap();

            assert_eq!(remaining, 0, "{operation} must cascade to its own records");
        }

        assert_eq!(
            invoke(&db, "tui:one", "list_databases", json!({})).unwrap()["databases"],
            if operation == "drop_database" { json!([]) } else { json!(["contacts"]) }
        );

        assert_eq!(invoke(&db, "tui:two", "describe_table", json!({})).unwrap(), schema);
        assert_eq!(invoke(&db, "tui:two", "query", json!({})).unwrap()["items"], json!([two]));

        assert_eq!(
            invoke(&db, "tui:two", "list_databases", json!({})).unwrap()["databases"],
            json!(["contacts"])
        );

        assert_eq!(
            invoke(&db, "tui:two", "list_tables", json!({})).unwrap()["tables"],
            json!(["person"])
        );
    }
}

fn records(db: &Arc<Mutex<Connection>>) -> Vec<(i64, String, i64, i64)> {
    db.lock()
        .unwrap()
        .prepare("SELECT id, value, created, updated FROM data_record ORDER BY id")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

#[test]
fn column_removal_rolls_back_records_and_schema_then_retries() {
    let db = tests::db();

    let one = populate(&db, "tui:one", "Ada");

    let two =
        invoke(&db, "tui:one", "insert", json!({"record": {"name": "Grace", "age": 85}})).unwrap();

    let three = invoke(&db, "tui:one", "insert", json!({"record": {"name": "Lin"}})).unwrap();

    db.lock()
        .unwrap()
        .execute_batch(
            "UPDATE data_record SET created = 6, updated = 7;

             CREATE TRIGGER reject_schema BEFORE UPDATE OF columns ON data_table BEGIN
                 SELECT CASE WHEN EXISTS (
                     SELECT 1 FROM data_record WHERE json_extract(value, '$.age') IS NOT NULL
                 ) THEN RAISE(ABORT, 'records were not rewritten') END;

                 SELECT RAISE(ABORT, 'injected schema failure');
             END;",
        )
        .unwrap();
    let before = records(&db);

    let schema = invoke(&db, "tui:one", "describe_table", json!({})).unwrap();
    let error = invoke(&db, "tui:one", "drop_column", json!({"column_name": "age"})).unwrap_err();

    assert!(error.to_string().contains("injected schema failure"), "{error}");
    assert_eq!(records(&db), before, "values and timestamps must roll back together");
    assert_eq!(invoke(&db, "tui:one", "describe_table", json!({})).unwrap(), schema);

    db.lock().unwrap().execute_batch("DROP TRIGGER reject_schema;").unwrap();

    let result = invoke(&db, "tui:one", "drop_column", json!({"column_name": "age"})).unwrap();

    assert_eq!(result["records_updated"], 3);

    assert_eq!(
        invoke(&db, "tui:one", "query", json!({})).unwrap()["items"],
        json!([
            {"id": one["id"], "record": {"name": "Ada"}},
            {"id": two["id"], "record": {"name": "Grace"}},
            {"id": three["id"], "record": {"name": "Lin"}}
        ])
    );

    assert_eq!(
        invoke(&db, "tui:one", "describe_table", json!({})).unwrap()["columns"],
        json!([{"name": "name", "type": "text", "required": true}])
    );
}
