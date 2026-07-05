use super::default_db_path;
use super::record::{FdrRecord, PendingRecords};
use super::schema::{self, FdrTable};
use bachelor::channel::mpsc::MpscChannelConsumer;
use bachelor::error::Closed;
use futures::future::{Either, select};
use rootcause::prelude::*;
use std::pin::pin;
use std::time::Duration;
use tokio::time::MissedTickBehavior;
use turso::params::Params;
use turso::{Builder, Connection, Statement};

const FLUSH_INTERVAL: Duration = Duration::from_secs(30);

struct Inserts {
    session: Statement,
    usage_credit: Statement,
    activity: Statement,
    credit_window_state: Statement,
    pain_change: Statement,
    credit_limit_change: Statement,
    credit_event: Statement,
}

/// Durable record sink. Owns the database handle, connection, and the
/// long-lived prepared INSERT statements.
pub struct DataWriter {
    conn: Connection,
    inserts: Inserts,
    current_session_id: Option<i64>,
}

impl DataWriter {
    pub async fn new() -> Result<Self, Report> {
        let path = default_db_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| report!(e).context("create database parent directory"))?;
        }
        let path_str = path
            .to_str()
            .ok_or_else(|| report!("database path is not valid utf-8"))?;
        let db = Builder::new_local(path_str)
            .build()
            .await
            .map_err(|e| report!(e).context("open database"))?;
        let conn = db
            .connect()
            .map_err(|e| report!(e).context("connect to database"))?;
        schema::init(&conn).await?;
        let inserts = prepare_inserts(&conn).await?;
        Ok(Self {
            conn,
            inserts,
            current_session_id: None,
        })
    }

    #[cfg(test)]
    pub(crate) async fn new_in_memory() -> Result<Self, Report> {
        let db = Builder::new_local(":memory:")
            .build()
            .await
            .map_err(|e| report!(e).context("open in-memory database"))?;
        let conn = db
            .connect()
            .map_err(|e| report!(e).context("connect to database"))?;
        schema::init(&conn).await?;
        let inserts = prepare_inserts(&conn).await?;
        Ok(Self {
            conn,
            inserts,
            current_session_id: None,
        })
    }
}

async fn prepare_inserts(conn: &Connection) -> Result<Inserts, Report> {
    use super::record::{
        ActivityBucket, CreditEventRecord, CreditLimitChange, CreditWindowState, FdrSession,
        PainChange, UsageCreditBucket,
    };
    let session = prepare_table_insert::<FdrSession>(conn).await?;
    let usage_credit = prepare_table_insert::<UsageCreditBucket>(conn).await?;
    let activity = prepare_table_insert::<ActivityBucket>(conn).await?;
    let credit_window_state = prepare_table_insert::<CreditWindowState>(conn).await?;
    let pain_change = prepare_table_insert::<PainChange>(conn).await?;
    let credit_limit_change = prepare_table_insert::<CreditLimitChange>(conn).await?;
    let credit_event = prepare_table_insert::<CreditEventRecord>(conn).await?;
    Ok(Inserts {
        session,
        usage_credit,
        activity,
        credit_window_state,
        pain_change,
        credit_limit_change,
        credit_event,
    })
}

async fn prepare_table_insert<T: FdrTable>(conn: &Connection) -> Result<Statement, Report> {
    let statement = conn
        .prepare(T::INSERT)
        .await
        .map_err(|e| report!(e).context(format!("prepare {} insert", T::NAME)))?;
    Ok(statement)
}

/// Writer task. Owns the [`PendingRecords`] buffer, the [`DataWriter`], and
/// the flush timer. Buffers every received [`FdrRecord`] and flushes the
/// buffer in one transaction every [`FLUSH_INTERVAL`]. When the record
/// channel closes (all feeders dropped their senders) it flushes once more
/// and exits. A failed write or commit aborts the task with the error.
pub async fn writer_task(mut records: MpscChannelConsumer<FdrRecord>) -> Result<(), Report> {
    let mut writer = DataWriter::new().await?;
    let mut pending = PendingRecords::default();

    let mut interval = tokio::time::interval(FLUSH_INTERVAL);
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    interval.tick().await;

    loop {
        let recv = pin!(records.recv());
        let tick = pin!(interval.tick());
        match select(recv, tick).await {
            Either::Left((Ok(record), _)) => pending.push(record),
            Either::Left((Err(Closed), _)) => {
                flush_pending(&mut writer, &mut pending).await?;
                return Ok(());
            }
            Either::Right((_, _)) => flush_pending(&mut writer, &mut pending).await?,
        }
    }
}

async fn flush_pending(w: &mut DataWriter, pending: &mut PendingRecords) -> Result<(), Report> {
    if pending.is_empty() {
        return Ok(());
    }

    let tx = w
        .conn
        .transaction()
        .await
        .map_err(|e| report!(e).context("begin write transaction"))?;

    for record in &pending.sessions {
        insert_row(&mut w.inserts.session, record.as_ref(), 0).await?;
        w.current_session_id = Some(tx.last_insert_rowid());
    }

    let sid = w
        .current_session_id
        .ok_or_else(|| report!("non-session record arrived before any fdr_session"))?;

    for record in &pending.usage_credit {
        insert_row(&mut w.inserts.usage_credit, record.as_ref(), sid).await?;
    }
    for record in &pending.activity {
        insert_row(&mut w.inserts.activity, record, sid).await?;
    }
    for record in &pending.credit_window_states {
        insert_row(&mut w.inserts.credit_window_state, record, sid).await?;
    }
    for record in &pending.pain_changes {
        insert_row(&mut w.inserts.pain_change, record.as_ref(), sid).await?;
    }
    for record in &pending.credit_limit_changes {
        insert_row(&mut w.inserts.credit_limit_change, record, sid).await?;
    }
    for record in &pending.credit_events {
        insert_row(&mut w.inserts.credit_event, record, sid).await?;
    }

    tx.commit()
        .await
        .map_err(|e| report!(e).context("commit write transaction"))?;

    pending.clear();
    Ok(())
}

/// Execute one prepared INSERT for an `FdrTable` row. Takes `&mut Statement`
/// (not `&mut DataWriter`) so it composes with the open `Transaction`'s
/// mutable borrow of the connection.
async fn insert_row<T: FdrTable>(stmt: &mut Statement, record: &T, sid: i64) -> Result<(), Report> {
    stmt.execute(Params::Positional(record.values(sid)))
        .await
        .map_err(|e| report!(e).context(format!("insert {}", T::NAME)))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credit::utilization::CreditKind;
    use crate::fdr::record::{
        ActivityBucket, CreditEventKind, CreditEventRecord, CreditLimitChange, CreditWindowState,
        FdrSession, PainChange, PendingRecords, UsageCreditBucket,
    };
    use crate::pain::PainBias;
    use jiff::Timestamp;
    use shared::model::Credit;
    use std::time::Duration;
    use turso::Value;
    use uuid::Uuid;

    fn block_on<F: std::future::Future>(fut: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime")
            .block_on(fut)
    }

    #[test]
    fn init_schema_is_idempotent() {
        block_on(async {
            let writer = DataWriter::new_in_memory().await.expect("open writer");
            // Init already ran inside new_in_memory; running it again must
            // trust the stamped schema version and return without replaying
            // DDL.
            schema::init(&writer.conn).await.expect("re-init");

            let mut rows = writer
                .conn
                .query("PRAGMA user_version", ())
                .await
                .expect("pragma query");
            let row = rows.next().await.expect("rows").expect("one row");
            assert_eq!(row.get_value(0).unwrap(), Value::Integer(1));
        });
    }

    #[test]
    fn init_schema_trusts_existing_version_one() {
        block_on(async {
            let db = Builder::new_local(":memory:")
                .build()
                .await
                .expect("open db");
            let conn = db.connect().expect("connect");
            conn.pragma_update("user_version", 1_u32)
                .await
                .expect("stamp version");

            schema::init(&conn).await.expect("init");

            let mut rows = conn
                .query(
                    "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'fdr_session'",
                    (),
                )
                .await
                .expect("query schema");
            assert!(rows.next().await.expect("rows").is_none());
        });
    }

    #[test]
    fn init_schema_rejects_unsupported_user_version() {
        block_on(async {
            let db = Builder::new_local(":memory:")
                .build()
                .await
                .expect("open db");
            let conn = db.connect().expect("connect");
            conn.pragma_update("user_version", 2_u32)
                .await
                .expect("stamp version");

            let err = schema::init(&conn)
                .await
                .expect_err("unsupported version should fail");
            assert!(
                err.to_string()
                    .contains("unsupported schema user_version 2")
            );
        });
    }

    #[test]
    fn roundtrip_one_of_each_record_type() {
        block_on(async {
            let mut writer = DataWriter::new_in_memory().await.expect("open writer");
            let mut pending = PendingRecords::default();

            let started_at = Timestamp::from_second(1_700_000_000).unwrap();
            let session_uuid = Uuid::from_u128(0x0123_4567_89ab_cdef_0123_4567_89ab_cdef);
            pending.sessions.push(Box::new(FdrSession {
                session_uuid,
                started_at,
                app_state_id: Uuid::from_u128(0xfedc_ba98_7654_3210_fedc_ba98_7654_3210),
                app_state_basis: 7,
                app_state_generation: 42,
                activity_total_baseline: Duration::from_secs(60),
            }));

            pending.usage_credit.push(Box::new(UsageCreditBucket {
                bucket_start: started_at,
                bucket_end: started_at + Duration::from_secs(5),
                increment_count: 3,
                u_click_count: 0,
                u_drag: Duration::ZERO,
                u_key_left_count: 0,
                u_key_right_count: 0,
                u_key_other_count: 0,
                u_left_combo_count: 0,
                u_right_combo_count: 0,
                u_cross_combo_count: 0,
                u_other_combo_count: 0,
                u_scroll_count: 0,
                u_left_shift: Duration::ZERO,
                u_left_ctrl: Duration::ZERO,
                u_left_alt: Duration::ZERO,
                u_left_meta: Duration::ZERO,
                u_left_multi: Duration::ZERO,
                u_right_shift: Duration::ZERO,
                u_right_ctrl: Duration::ZERO,
                u_right_alt: Duration::ZERO,
                u_right_meta: Duration::ZERO,
                u_right_multi: Duration::ZERO,
                u_active: Duration::ZERO,
                cb_click: Credit::default(),
                cb_drag: Credit::default(),
                cb_key_left: Credit::default(),
                cb_key_right: Credit::default(),
                cb_key_other: Credit::default(),
                cb_key_left_combo: Credit::default(),
                cb_key_right_combo: Credit::default(),
                cb_key_cross_combo: Credit::default(),
                cb_key_other_combo: Credit::default(),
                cb_scroll: Credit::default(),
                cb_left_shift: Credit::default(),
                cb_left_ctrl: Credit::default(),
                cb_left_alt: Credit::default(),
                cb_left_meta: Credit::default(),
                cb_left_multi: Credit::default(),
                cb_right_shift: Credit::default(),
                cb_right_ctrl: Credit::default(),
                cb_right_alt: Credit::default(),
                cb_right_meta: Credit::default(),
                cb_right_multi: Credit::default(),
                cx_click: Credit::default(),
                cx_drag: Credit::default(),
                cx_key_left: Credit::default(),
                cx_key_right: Credit::default(),
                cx_key_other: Credit::default(),
                cx_key_left_combo: Credit::default(),
                cx_key_right_combo: Credit::default(),
                cx_key_cross_combo: Credit::default(),
                cx_key_other_combo: Credit::default(),
                cx_scroll: Credit::default(),
                cx_left_shift: Credit::default(),
                cx_left_ctrl: Credit::default(),
                cx_left_alt: Credit::default(),
                cx_left_meta: Credit::default(),
                cx_left_multi: Credit::default(),
                cx_right_shift: Credit::default(),
                cx_right_ctrl: Credit::default(),
                cx_right_alt: Credit::default(),
                cx_right_meta: Credit::default(),
                cx_right_multi: Credit::default(),
                max_increment_key_left_count: 5,
                max_increment_key_right_count: 4,
                max_increment_key_other_count: 2,
                max_increment_left_combo_count: 3,
                max_increment_right_combo_count: 2,
                max_increment_cross_combo_count: 2,
                max_increment_other_combo_count: 1,
                max_increment_click_count: 4,
                max_increment_scroll_count: 2,
                max_increment_total_credit: Credit::default(),
                sum_increment_total_credit_squared: Credit::default(),
                active_increment_count: 1,
                observed_duration: Duration::from_secs(5),
            }));

            pending.activity.push(ActivityBucket {
                bucket_start: started_at,
                bucket_end: started_at + Duration::from_secs(30),
                activity_delta: Duration::from_secs(7),
            });

            pending.credit_window_states.push(CreditWindowState {
                recorded_at: started_at,
                rest_credit_total: 1.5,
                break_credit_total: 2.5,
                day_credit_total: 3.5,
            });

            pending.pain_changes.push(Box::new(PainChange {
                recorded_at: started_at,
                label: "wrist".to_owned(),
                bias: PainBias::Right,
                ratio: 0.25,
                last_updated: started_at + Duration::from_secs(1),
            }));

            pending.credit_limit_changes.push(CreditLimitChange {
                recorded_at: started_at,
                rest: 10.0,
                breaks: 20.0,
                day: 30.0,
            });

            pending.credit_events.push(CreditEventRecord {
                recorded_at: started_at,
                kind: CreditKind::Breaks,
                event: CreditEventKind::Escalation,
                level: Some(2),
                rest_utilization: 0.1,
                break_utilization: 0.5,
                day_utilization: 0.9,
            });

            flush_pending(&mut writer, &mut pending)
                .await
                .expect("flush");
            assert!(pending.is_empty());
            let sid = writer.current_session_id.expect("session id");
            assert_eq!(sid, 1);

            // fdr_session: session_uuid round-trips as 16-byte BLOB.
            let mut rows = writer
                .conn
                .query("SELECT id, session_uuid FROM fdr_session", ())
                .await
                .expect("query session");
            let row = rows.next().await.expect("rows").expect("one row");
            assert_eq!(row.get_value(0).unwrap(), Value::Integer(sid));
            assert_eq!(
                row.get_value(1).unwrap(),
                Value::Blob(session_uuid.as_bytes().to_vec()),
            );
            assert!(rows.next().await.unwrap().is_none());

            // usage_credit: FK + a representative aggregate scalar.
            let mut rows = writer
                .conn
                .query(
                    "SELECT session_id, increment_count, observed_duration_ns \
                     FROM usage_credit",
                    (),
                )
                .await
                .expect("query usage");
            let row = rows.next().await.expect("rows").expect("one row");
            assert_eq!(row.get_value(0).unwrap(), Value::Integer(sid));
            assert_eq!(row.get_value(1).unwrap(), Value::Integer(3));
            assert_eq!(row.get_value(2).unwrap(), Value::Integer(5_000_000_000),);

            // activity: FK + duration.
            let mut rows = writer
                .conn
                .query("SELECT session_id, activity_delta_ns FROM activity", ())
                .await
                .expect("query activity");
            let row = rows.next().await.expect("rows").expect("one row");
            assert_eq!(row.get_value(0).unwrap(), Value::Integer(sid));
            assert_eq!(row.get_value(1).unwrap(), Value::Integer(7_000_000_000),);

            // credit_window_state: FK + a real.
            let mut rows = writer
                .conn
                .query(
                    "SELECT session_id, day_credit_total FROM credit_window_state",
                    (),
                )
                .await
                .expect("query window");
            let row = rows.next().await.expect("rows").expect("one row");
            assert_eq!(row.get_value(0).unwrap(), Value::Integer(sid));
            assert_eq!(row.get_value(1).unwrap(), Value::Real(3.5));

            // pain_change: FK + enum text in snake_case + label.
            let mut rows = writer
                .conn
                .query("SELECT session_id, label, bias FROM pain_change", ())
                .await
                .expect("query pain");
            let row = rows.next().await.expect("rows").expect("one row");
            assert_eq!(row.get_value(0).unwrap(), Value::Integer(sid));
            assert_eq!(row.get_value(1).unwrap(), Value::Text("wrist".to_owned()));
            assert_eq!(row.get_value(2).unwrap(), Value::Text("right".to_owned()));

            // credit_limit_change: FK + reals.
            let mut rows = writer
                .conn
                .query(
                    "SELECT session_id, rest, breaks, day FROM credit_limit_change",
                    (),
                )
                .await
                .expect("query limit");
            let row = rows.next().await.expect("rows").expect("one row");
            assert_eq!(row.get_value(0).unwrap(), Value::Integer(sid));
            assert_eq!(row.get_value(1).unwrap(), Value::Real(10.0));
            assert_eq!(row.get_value(2).unwrap(), Value::Real(20.0));
            assert_eq!(row.get_value(3).unwrap(), Value::Real(30.0));

            // credit_event: FK + enum text + Option<u8> as Integer.
            let mut rows = writer
                .conn
                .query(
                    "SELECT session_id, kind, event, level FROM credit_event",
                    (),
                )
                .await
                .expect("query event");
            let row = rows.next().await.expect("rows").expect("one row");
            assert_eq!(row.get_value(0).unwrap(), Value::Integer(sid));
            assert_eq!(row.get_value(1).unwrap(), Value::Text("breaks".to_owned()));
            assert_eq!(
                row.get_value(2).unwrap(),
                Value::Text("escalation".to_owned())
            );
            assert_eq!(row.get_value(3).unwrap(), Value::Integer(2));
        });
    }
}
