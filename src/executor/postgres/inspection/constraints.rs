use std::collections::BTreeMap;

use gaman_core::TRACKING_TABLE;
use gaman_core::states::{Constraint, Table};
use sqlx::PgConnection;

use super::{ExecutorError, PgCheckRow};

/// Retains constraints whose semantics are not fully represented by modeled table metadata.
pub(super) async fn attach_opaque_constraints(
    conn: &mut PgConnection,
    schemas: &[String],
    tables: &mut BTreeMap<i64, Table>,
) -> Result<(), ExecutorError> {
    // PostgreSQL 18 catalogs ordinary NOT NULL constraints as well as setting attnotnull.
    // Read conenforced via JSON so the query also parses on earlier server versions.
    let rows = sqlx::query_as::<_, PgCheckRow>(
        "SELECT rel.oid::int8 AS table_oid, con.conname AS constraint_name, \
         pg_get_constraintdef(con.oid, true) AS definition \
         FROM pg_constraint con \
         JOIN pg_class rel ON rel.oid = con.conrelid \
         JOIN pg_namespace ns ON ns.oid = rel.relnamespace \
         WHERE con.contype NOT IN ('p', 'u', 'f', 'c') \
         AND con.conrelid != 0 AND ns.nspname = ANY($1) \
         AND rel.relname != $2 \
         AND NOT (con.contype = 'n' AND con.convalidated AND NOT con.connoinherit \
             AND con.conislocal AND con.coninhcount = 0 AND con.conparentid = 0 \
             AND (to_jsonb(con)->>'conenforced') IS NOT DISTINCT FROM 'true' \
             AND EXISTS (SELECT 1 FROM pg_attribute a WHERE a.attrelid = con.conrelid \
                 AND a.attnum > 0 AND NOT a.attisdropped AND a.attnotnull \
                 AND con.conkey = ARRAY[a.attnum])) \
         ORDER BY rel.oid, con.conname",
    )
    .bind(schemas)
    .bind(TRACKING_TABLE)
    .fetch_all(conn)
    .await
    .map_err(|error| ExecutorError::Fetch(error.to_string()))?;

    for row in rows {
        if let Some(table) = tables.get_mut(&row.table_oid) {
            table.constraints.push(Constraint::from_trusted_raw(
                row.constraint_name,
                row.definition,
            ));
        }
    }
    Ok(())
}
