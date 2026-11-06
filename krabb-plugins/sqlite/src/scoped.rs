use super::*;

pub(super) fn inherit(
    source: &Connection,
    destination: &mut Connection,
    scope: &str,
) -> krabb_core::Result<()> {
    let result = (|| -> rusqlite::Result<()> {
        destination.execute_batch(
            "CREATE TABLE IF NOT EXISTS workspace_import (scope TEXT PRIMARY KEY)",
        )?;
        let transaction = destination.transaction_with_behavior(TransactionBehavior::Immediate)?;

        if transaction
            .query_row("SELECT 1 FROM workspace_import WHERE scope=?1", [scope], |_| Ok(()))
            .optional()?
            .is_some()
        {
            return Ok(());
        }

        let exists: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM data_database WHERE scope=?1)",
            [scope],
            |row| row.get(0),
        )?;

        if !exists {
            let snapshot = source.unchecked_transaction()?;

            for (table, columns, count) in [
                ("data_database", "scope,name,created", 3),
                ("data_table", "scope,database_name,name,columns,created", 5),
                ("data_record", "id,scope,database_name,table_name,value,created,updated", 7),
            ] {
                let mut statement =
                    snapshot.prepare(&format!("SELECT {columns} FROM {table} WHERE scope=?1"))?;
                let mut rows = statement.query([scope])?;
                let placeholders = vec!["?"; count].join(",");

                while let Some(row) = rows.next()? {
                    let values = (0..count)
                        .map(|index| row.get::<_, rusqlite::types::Value>(index))
                        .collect::<rusqlite::Result<Vec<_>>>()?;
                    transaction.execute(
                        &format!("INSERT INTO {table} ({columns}) VALUES ({placeholders})"),
                        rusqlite::params_from_iter(values),
                    )?;
                }
            }

            snapshot.commit()?;
        }

        transaction.execute("INSERT INTO workspace_import(scope) VALUES (?1)", [scope])?;
        transaction.commit()
    })();
    result.map_err(|error| denied(format!("Scoped data import failed: {error}")))
}
