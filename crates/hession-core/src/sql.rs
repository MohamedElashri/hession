//! Compatibility helpers for the existing desktop JSON response shapes.
use rusqlite::{
    types::{ToSql, Value as SqlValue, ValueRef},
    Connection,
};
use serde_json::{json, Map, Value};

fn ref_to_json(value: ValueRef<'_>) -> Value {
    match value {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(n) => json!(n),
        ValueRef::Real(n) => json!(n),
        ValueRef::Text(text) => json!(String::from_utf8_lossy(text).into_owned()),
        ValueRef::Blob(blob) => json!(String::from_utf8_lossy(blob).into_owned()),
    }
}

fn row_to_json(cols: &[String], row: &rusqlite::Row<'_>) -> Result<Value, rusqlite::Error> {
    let mut map = Map::new();
    for (index, name) in cols.iter().enumerate() {
        map.insert(name.clone(), ref_to_json(row.get_ref(index)?));
    }
    Ok(Value::Object(map))
}

/// `SELECT` returning every row as `serde_json::Value` objects keyed by
/// column name. Params are owned [`SqlValue`]s so callers can mix text/ints.
pub fn query_all(conn: &Connection, sql: &str, params: &[SqlValue]) -> Result<Vec<Value>, String> {
    let refs: Vec<&dyn ToSql> = params.iter().map(|v| v as &dyn ToSql).collect();
    let mut stmt = conn
        .prepare_cached(sql)
        .map_err(|e| format!("Could not prepare query: {e}"))?;
    let cols: Vec<String> = stmt.column_names().map(|s| s.to_string()).collect();
    let mapped = stmt
        .query_map(refs.as_slice(), |row| row_to_json(&cols, row))
        .map_err(|e| format!("Query failed: {e}"))?;
    let mut out = Vec::new();
    for row in mapped {
        out.push(row.map_err(|e| format!("Could not decode row: {e}"))?);
    }
    Ok(out)
}

/// `SELECT` expecting zero or one row.
pub fn query_one(
    conn: &Connection,
    sql: &str,
    params: &[SqlValue],
) -> Result<Option<Value>, String> {
    let mut rows = query_all(conn, sql, params)?;
    Ok(rows.pop())
}
