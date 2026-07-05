use crate::activity::ActivityState;
use crate::credit::utilization::CreditKind;
use crate::pain::PainBias;
use crate::persistence::AppStateIdentity;
use jiff::Timestamp;
use shared::model::Credit;
use std::time::Duration;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FdrSession {
    pub session_uuid: Uuid,
    pub started_at: Timestamp,
    pub app_state_id: Uuid,
    pub app_state_basis: u64,
    pub app_state_generation: u64,
    pub activity_total_baseline: Duration,
}

impl FdrSession {
    pub fn new(activity: &ActivityState, identity: &AppStateIdentity) -> Self {
        Self {
            session_uuid: Uuid::new_v4(),
            started_at: Timestamp::now(),
            app_state_id: identity.app_state_id,
            app_state_basis: identity.app_state_basis,
            app_state_generation: identity.app_state_generation,
            activity_total_baseline: activity.total(),
        }
    }
}

/// One record per completed ~5-second logical bucket, built from raw
/// `(UsageIncrement, CreditIncrement)` messages.
#[derive(Debug, Clone)]
pub struct UsageCreditBucket {
    pub bucket_start: Timestamp,
    pub bucket_end: Timestamp,
    pub increment_count: u32,
    pub u_click_count: u64,
    pub u_drag: Duration,
    pub u_key_left_count: u64,
    pub u_key_right_count: u64,
    pub u_key_other_count: u64,
    pub u_left_combo_count: u64,
    pub u_right_combo_count: u64,
    pub u_cross_combo_count: u64,
    pub u_other_combo_count: u64,
    pub u_scroll_count: u64,
    pub u_left_shift: Duration,
    pub u_left_ctrl: Duration,
    pub u_left_alt: Duration,
    pub u_left_meta: Duration,
    pub u_left_multi: Duration,
    pub u_right_shift: Duration,
    pub u_right_ctrl: Duration,
    pub u_right_alt: Duration,
    pub u_right_meta: Duration,
    pub u_right_multi: Duration,
    pub u_active: Duration,
    pub cb_click: Credit,
    pub cb_drag: Credit,
    pub cb_key_left: Credit,
    pub cb_key_right: Credit,
    pub cb_key_other: Credit,
    pub cb_key_left_combo: Credit,
    pub cb_key_right_combo: Credit,
    pub cb_key_cross_combo: Credit,
    pub cb_key_other_combo: Credit,
    pub cb_scroll: Credit,
    pub cb_left_shift: Credit,
    pub cb_left_ctrl: Credit,
    pub cb_left_alt: Credit,
    pub cb_left_meta: Credit,
    pub cb_left_multi: Credit,
    pub cb_right_shift: Credit,
    pub cb_right_ctrl: Credit,
    pub cb_right_alt: Credit,
    pub cb_right_meta: Credit,
    pub cb_right_multi: Credit,
    pub cx_click: Credit,
    pub cx_drag: Credit,
    pub cx_key_left: Credit,
    pub cx_key_right: Credit,
    pub cx_key_other: Credit,
    pub cx_key_left_combo: Credit,
    pub cx_key_right_combo: Credit,
    pub cx_key_cross_combo: Credit,
    pub cx_key_other_combo: Credit,
    pub cx_scroll: Credit,
    pub cx_left_shift: Credit,
    pub cx_left_ctrl: Credit,
    pub cx_left_alt: Credit,
    pub cx_left_meta: Credit,
    pub cx_left_multi: Credit,
    pub cx_right_shift: Credit,
    pub cx_right_ctrl: Credit,
    pub cx_right_alt: Credit,
    pub cx_right_meta: Credit,
    pub cx_right_multi: Credit,
    pub max_increment_key_left_count: u64,
    pub max_increment_key_right_count: u64,
    pub max_increment_key_other_count: u64,
    pub max_increment_left_combo_count: u64,
    pub max_increment_right_combo_count: u64,
    pub max_increment_cross_combo_count: u64,
    pub max_increment_other_combo_count: u64,
    pub max_increment_click_count: u64,
    pub max_increment_scroll_count: u64,
    pub max_increment_total_credit: Credit,
    pub sum_increment_total_credit_squared: Credit,
    pub active_increment_count: u32,
    pub observed_duration: Duration,
}

/// One record per 30-second activity sampling interval, derived from the
/// cumulative `ActivityState.total()`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ActivityBucket {
    pub bucket_start: Timestamp,
    pub bucket_end: Timestamp,
    pub activity_delta: Duration,
}

/// One record when the app's accumulated rest/break/day credit totals change.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CreditWindowState {
    pub recorded_at: Timestamp,
    pub rest_credit_total: f64,
    pub break_credit_total: f64,
    pub day_credit_total: f64,
}

/// One record when a label's committed (debounced) pain ratio changes.
#[derive(Debug, Clone, PartialEq)]
pub struct PainChange {
    pub recorded_at: Timestamp,
    pub label: String,
    pub bias: PainBias,
    pub ratio: f64,
    pub last_updated: Timestamp,
}

/// One record when credit limits change. Independent of the accumulated
/// credit-window totals.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CreditLimitChange {
    pub recorded_at: Timestamp,
    pub rest: f64,
    pub breaks: f64,
    pub day: f64,
}

/// Discriminator for the discrete credit-event variants, flattened off the
/// `CreditEvent` payload so the level is carried separately.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreditEventKind {
    Reached,
    Escalation,
    Reset,
}

impl CreditEventKind {
    pub fn as_str(self) -> &'static str {
        match self {
            CreditEventKind::Reached => "reached",
            CreditEventKind::Escalation => "escalation",
            CreditEventKind::Reset => "reset",
        }
    }
}

/// One record per `CreditEvent` received from the utilization driver.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CreditEventRecord {
    pub recorded_at: Timestamp,
    pub kind: CreditKind,
    pub event: CreditEventKind,
    pub level: Option<u8>,
    pub rest_utilization: f64,
    pub break_utilization: f64,
    pub day_utilization: f64,
}

/// Internal record channel payload. The larger variants are boxed so the
/// channel moves pointer-sized payloads for records with larger nested
/// fields; the smaller records stay inline.
#[derive(Debug, Clone)]
pub enum FdrRecord {
    Session(Box<FdrSession>),
    UsageCredit(Box<UsageCreditBucket>),
    Activity(ActivityBucket),
    CreditWindowState(CreditWindowState),
    PainChange(Box<PainChange>),
    CreditLimitChange(CreditLimitChange),
    CreditEvent(CreditEventRecord),
}

/// In-memory buffer of fully constructed records awaiting the next flush.
/// Owned solely by the writer task, which unpacks incoming [`FdrRecord`]
/// values into these per-type Vecs. Large records stay boxed; there is no
/// reason to unbox them just to store them before writing.
//
// `clippy::vec_box`: the boxing is intentional. The large variants are boxed
// in `FdrRecord` so the channel moves pointer-sized payloads, and they stay
// boxed here so the writer can store them without unboxing before a write.
#[derive(Debug, Default)]
#[allow(clippy::vec_box)]
pub struct PendingRecords {
    pub sessions: Vec<Box<FdrSession>>,
    pub usage_credit: Vec<Box<UsageCreditBucket>>,
    pub activity: Vec<ActivityBucket>,
    pub credit_window_states: Vec<CreditWindowState>,
    pub pain_changes: Vec<Box<PainChange>>,
    pub credit_limit_changes: Vec<CreditLimitChange>,
    pub credit_events: Vec<CreditEventRecord>,
}

impl PendingRecords {
    /// Unpack a received [`FdrRecord`] into the matching per-type buffer.
    pub fn push(&mut self, record: FdrRecord) {
        match record {
            FdrRecord::Session(record) => self.sessions.push(record),
            FdrRecord::UsageCredit(record) => self.usage_credit.push(record),
            FdrRecord::Activity(record) => self.activity.push(record),
            FdrRecord::CreditWindowState(record) => self.credit_window_states.push(record),
            FdrRecord::PainChange(record) => self.pain_changes.push(record),
            FdrRecord::CreditLimitChange(record) => self.credit_limit_changes.push(record),
            FdrRecord::CreditEvent(record) => self.credit_events.push(record),
        }
    }

    /// `true` when no records are buffered.
    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
            && self.usage_credit.is_empty()
            && self.activity.is_empty()
            && self.credit_window_states.is_empty()
            && self.pain_changes.is_empty()
            && self.credit_limit_changes.is_empty()
            && self.credit_events.is_empty()
    }

    /// Drop every buffered record. Called by the writer only after a flush
    /// transaction commits successfully.
    pub fn clear(&mut self) {
        self.sessions.clear();
        self.usage_credit.clear();
        self.activity.clear();
        self.credit_window_states.clear();
        self.pain_changes.clear();
        self.credit_limit_changes.clear();
        self.credit_events.clear();
    }
}
