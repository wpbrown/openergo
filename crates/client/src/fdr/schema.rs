use super::record::{
    ActivityBucket, CreditEventRecord, CreditLimitChange, CreditWindowState, FdrSession,
    PainChange, UsageCreditBucket,
};
use jiff::Timestamp;
use rootcause::prelude::*;
use shared::model::Credit;
use std::time::Duration;
use turso::{Connection, Value};
use uuid::Uuid;

pub trait FdrTable {
    const NAME: &'static str;
    const CREATE_TABLE: &'static str;
    const CREATE_INDEX: &'static [&'static str];
    const INSERT: &'static str;
    fn values(&self, session_id: i64) -> Vec<Value>;
}

fn ts_value(ts: Timestamp) -> Value {
    Value::Integer(i64::try_from(ts.as_nanosecond()).expect("timestamp fits in i64 ns"))
}

fn dur_value(d: Duration) -> Value {
    Value::Integer(i64::try_from(d.as_nanos()).unwrap_or(i64::MAX))
}

fn credit_value(c: Credit) -> Value {
    Value::Real(c.as_f64())
}

fn uuid_value(u: Uuid) -> Value {
    Value::Blob(u.as_bytes().to_vec())
}

fn text_value(s: &str) -> Value {
    Value::Text(s.to_owned())
}

fn opt_u8_value(v: Option<u8>) -> Value {
    match v {
        Some(n) => Value::Integer(n as i64),
        None => Value::Null,
    }
}

include!("schema.generated.rs");

// ---- schema initialization ----

const SCHEMA_VERSION: u32 = 1;

/// Ensure the FDR schema is present and at the supported version.
///
/// A `user_version` of 0 means no FDR schema has been stamped yet, so the
/// initial schema is applied. Version 1 is trusted as-is. Newer or otherwise
/// unexpected versions are rejected until migrations exist.
pub async fn init(conn: &Connection) -> Result<(), Report> {
    match user_version(conn).await? {
        0 => init_v1(conn).await,
        SCHEMA_VERSION => Ok(()),
        version => Err(report!("unsupported schema user_version {version}")),
    }
}

async fn user_version(conn: &Connection) -> Result<u32, Report> {
    let mut rows = conn
        .query("PRAGMA user_version", ())
        .await
        .map_err(|e| report!(e).context("query schema user_version"))?;
    let row = rows
        .next()
        .await
        .map_err(|e| report!(e).context("read schema user_version"))?
        .ok_or_else(|| report!("PRAGMA user_version returned no rows"))?;
    match row.get_value(0).map_err(|e| report!(e))? {
        Value::Integer(version) => {
            u32::try_from(version).map_err(|_| report!("invalid schema user_version {version}"))
        }
        value => Err(report!(
            "PRAGMA user_version returned unexpected value {value:?}"
        )),
    }
}

/// Create every FDR table and index and stamp `user_version = 1`.
/// `fdr_session` is created first because the other tables reference it.
async fn init_v1(conn: &Connection) -> Result<(), Report> {
    let mut ddl = String::new();
    append_table_schema::<FdrSession>(&mut ddl);
    append_table_schema::<UsageCreditBucket>(&mut ddl);
    append_table_schema::<ActivityBucket>(&mut ddl);
    append_table_schema::<CreditWindowState>(&mut ddl);
    append_table_schema::<PainChange>(&mut ddl);
    append_table_schema::<CreditLimitChange>(&mut ddl);
    append_table_schema::<CreditEventRecord>(&mut ddl);

    conn.execute_batch(&ddl)
        .await
        .map_err(|e| report!(e).context("apply schema DDL"))?;

    conn.pragma_update("user_version", SCHEMA_VERSION)
        .await
        .map_err(|e| report!(e).context("set schema user_version"))?;

    Ok(())
}

fn append_table_schema<T: FdrTable>(ddl: &mut String) {
    ddl.push_str(T::CREATE_TABLE);
    ddl.push('\n');
    for stmt in T::CREATE_INDEX {
        ddl.push_str(stmt);
        ddl.push('\n');
    }
}
