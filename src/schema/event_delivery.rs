//! F-120 event delivery layer (`maestro.run_event_ack.v1` + `maestro.run_event_gap.v1`).
//!
//! Consumer state ONLY — never event state. `events.ndjson` (F-115) stays the
//! append-only source of truth and is never trimmed; F-117 resume and F-112 monitor
//! never read ack files. These types record what a local consumer has accepted
//! (ack high-water) and let a `balanced` live stream cover intentionally-shed
//! low-value (`activity`) seq ranges with a `run_event_gap` frame. Everything here
//! is ids / sequence numbers / closed enums / counters / timestamps — never a raw
//! prompt, log, payload, ref, path, env value, or user identity.

use anyhow::{anyhow, ensure, Result};
use serde::{Deserialize, Serialize};

/// How a live stream delivers events to a consumer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeliveryMode {
    /// Emit every `run_event` with `seq > cursor` — the F-115 default.
    Lossless,
    /// Emit every critical/normal event; cover shed `activity` spans with gaps.
    Balanced,
}

/// Per-event live-delivery class. The durable ledger keeps every class regardless.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunEventDeliveryClass {
    /// Never shed from live delivery (terminal / permission / audit / unknown).
    Critical,
    /// Retained in v1 (lifecycle); not shed by balanced mode.
    Normal,
    /// May be gap-covered under balanced backpressure.
    Activity,
}

/// Why a gap frame was emitted (closed; v1 only one reason).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GapReason {
    ActivityBackpressure,
}

/// Counters carried with an ack update — counts only, never labels/paths/text.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunEventAckStats {
    #[serde(default)]
    pub acked_events: u64,
    #[serde(default)]
    pub acked_gaps: u64,
    /// total seq count the consumer accepted through gaps.
    #[serde(default)]
    pub shed_events: u64,
}

/// `maestro.run_event_ack.v1` — a consumer's acknowledged high-water for one run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunEventAck {
    #[serde(default = "crate::schema::run_event_ack_version")]
    pub schema_version: String,
    pub run_id: String,
    pub consumer_id: String,
    /// largest seq the consumer accepted, including seqs covered by gap frames.
    pub high_water_seq: u64,
    /// largest seq the server saw in the ledger when this ack was written.
    pub last_seen_seq: u64,
    pub delivery: DeliveryMode,
    pub updated_at: String,
    #[serde(default)]
    pub stats: RunEventAckStats,
}

/// HTTP body for `POST /api/runs/:id/events/ack` (Step 2 wires the handler).
#[derive(Debug, Clone, Deserialize)]
pub struct RunEventAckRequest {
    pub consumer_id: String,
    pub high_water_seq: u64,
    #[serde(default)]
    pub delivery: Option<DeliveryMode>,
    #[serde(default)]
    pub acked_events: Option<u64>,
    #[serde(default)]
    pub acked_gaps: Option<u64>,
    #[serde(default)]
    pub shed_events: Option<u64>,
}

/// `maestro.run_event_gap.v1` — an SSE control frame for an intentionally-shed
/// contiguous `activity` seq range. Emitted only by `delivery=balanced`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunEventGap {
    #[serde(default = "crate::schema::run_event_gap_version")]
    pub schema_version: String,
    pub run_id: String,
    pub from_seq: u64,
    pub to_seq: u64,
    pub count: u64,
    pub reason: GapReason,
    pub delivery: DeliveryMode,
    /// the delivery classes shed in this range; v1 only `[activity]`.
    pub classes: Vec<RunEventDeliveryClass>,
}

impl RunEventGap {
    /// A v1 activity-backpressure gap covering the inclusive `[from, to]` seq range.
    /// `from <= to` is a precondition (the planner guarantees it); `count` is
    /// computed with saturating arithmetic so an out-of-range caller never panics or
    /// wraps — `validate_gap` then rejects any inconsistent result.
    pub fn activity(run_id: impl Into<String>, from_seq: u64, to_seq: u64) -> Self {
        Self {
            schema_version: crate::schema::run_event_gap_version(),
            run_id: run_id.into(),
            from_seq,
            to_seq,
            count: to_seq.saturating_sub(from_seq).saturating_add(1),
            reason: GapReason::ActivityBackpressure,
            delivery: DeliveryMode::Balanced,
            classes: vec![RunEventDeliveryClass::Activity],
        }
    }
}

fn validate_id(field: &str, value: &str) -> Result<()> {
    crate::paths::validate_path_component(field, value)
}

/// Validate an ack record before-write and after-read (a hand-edited ack that
/// smuggles an inconsistent high-water or an unsafe id is corrupt, not served).
pub fn validate_ack(ack: &RunEventAck) -> Result<()> {
    ensure!(
        ack.schema_version == crate::schema::RUN_EVENT_ACK_V1,
        "run_event_ack schema_version {:?} is not {}",
        ack.schema_version,
        crate::schema::RUN_EVENT_ACK_V1
    );
    validate_id("run id", &ack.run_id)?;
    validate_id("event consumer id", &ack.consumer_id)?;
    ensure!(
        chrono::DateTime::parse_from_rfc3339(&ack.updated_at).is_ok(),
        "run_event_ack updated_at {:?} is not RFC3339",
        ack.updated_at
    );
    ensure!(
        ack.high_water_seq <= ack.last_seen_seq,
        "run_event_ack high_water_seq {} exceeds last_seen_seq {}",
        ack.high_water_seq,
        ack.last_seen_seq
    );
    Ok(())
}

/// Validate a gap control frame: closed v1 shape (balanced + activity_backpressure +
/// classes exactly `[activity]`) with a self-consistent, overflow-checked range.
pub fn validate_gap(gap: &RunEventGap) -> Result<()> {
    ensure!(
        gap.schema_version == crate::schema::RUN_EVENT_GAP_V1,
        "run_event_gap schema_version {:?} is not {}",
        gap.schema_version,
        crate::schema::RUN_EVENT_GAP_V1
    );
    validate_id("run id", &gap.run_id)?;
    // v1 gaps cover only activity shed under balanced backpressure; any other
    // delivery / reason / class combination is corrupt, not a valid gap.
    ensure!(
        gap.delivery == DeliveryMode::Balanced,
        "run_event_gap delivery must be balanced"
    );
    ensure!(
        gap.reason == GapReason::ActivityBackpressure,
        "run_event_gap reason must be activity_backpressure"
    );
    ensure!(
        gap.classes == [RunEventDeliveryClass::Activity],
        "run_event_gap classes must be exactly [activity], got {:?}",
        gap.classes
    );
    // checked arithmetic: `from > to` and an overflowing range are errors, never a
    // debug panic or a release wrap.
    let span = gap.to_seq.checked_sub(gap.from_seq).ok_or_else(|| {
        anyhow!(
            "run_event_gap from_seq {} > to_seq {}",
            gap.from_seq,
            gap.to_seq
        )
    })?;
    let expected = span.checked_add(1).ok_or_else(|| {
        anyhow!(
            "run_event_gap range {}..={} overflows",
            gap.from_seq,
            gap.to_seq
        )
    })?;
    ensure!(
        gap.count == expected,
        "run_event_gap count {} != range {}..={}",
        gap.count,
        gap.from_seq,
        gap.to_seq
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ack() -> RunEventAck {
        RunEventAck {
            schema_version: crate::schema::run_event_ack_version(),
            run_id: "r-1".into(),
            consumer_id: "webui-main".into(),
            high_water_seq: 10,
            last_seen_seq: 12,
            delivery: DeliveryMode::Balanced,
            updated_at: "2026-06-05T00:00:00Z".into(),
            stats: RunEventAckStats {
                acked_events: 5,
                acked_gaps: 1,
                shed_events: 4,
            },
        }
    }

    #[test]
    fn ack_round_trips_and_validates() {
        let a = ack();
        assert!(validate_ack(&a).is_ok());
        let json = serde_json::to_string(&a).unwrap();
        assert!(json.contains("\"schema_version\":\"maestro.run_event_ack.v1\""));
        assert!(json.contains("\"delivery\":\"balanced\""));
        assert_eq!(serde_json::from_str::<RunEventAck>(&json).unwrap(), a);
    }

    #[test]
    fn ack_rejects_bad_schema_ids_time_and_highwater() {
        let mut a = ack();
        a.schema_version = "maestro.run_event_ack.v2".into();
        assert!(validate_ack(&a).is_err());

        for bad in ["../escape", "a/b", ".."] {
            let mut a = ack();
            a.consumer_id = bad.into();
            assert!(validate_ack(&a).is_err(), "consumer {bad:?} must reject");
            let mut a = ack();
            a.run_id = bad.into();
            assert!(validate_ack(&a).is_err());
        }
        let mut a = ack();
        a.updated_at = "nope".into();
        assert!(validate_ack(&a).is_err());

        // high_water cannot exceed last_seen
        let mut a = ack();
        a.high_water_seq = 99;
        assert!(validate_ack(&a).is_err());
    }

    #[test]
    fn gap_constructor_validates_and_serializes_closed() {
        let g = RunEventGap::activity("r-1", 4, 7);
        assert_eq!(g.count, 4);
        assert!(validate_gap(&g).is_ok());
        let json = serde_json::to_string(&g).unwrap();
        assert!(json.contains("\"reason\":\"activity_backpressure\""));
        assert!(json.contains("\"delivery\":\"balanced\""));
        assert!(json.contains("\"classes\":[\"activity\"]"));
        // no event payload / message / path fields exist on a gap
        assert!(!json.contains("payload") && !json.contains("message"));
    }

    #[test]
    fn gap_rejects_bad_range_count_or_empty_classes() {
        let mut g = RunEventGap::activity("r-1", 4, 7);
        g.from_seq = 7; // from > to
        g.to_seq = 4;
        assert!(validate_gap(&g).is_err());

        let mut g = RunEventGap::activity("r-1", 4, 7);
        g.count = 2; // wrong count
        assert!(validate_gap(&g).is_err());

        let mut g = RunEventGap::activity("r-1", 4, 7);
        g.classes.clear();
        assert!(validate_gap(&g).is_err());

        let mut g = RunEventGap::activity("../x", 4, 7); // unsafe run id
        g.run_id = "../x".into();
        assert!(validate_gap(&g).is_err());
    }

    #[test]
    fn gap_rejects_non_v1_delivery_class_combos_and_overflow() {
        // N1: a v1 gap must be balanced + activity_backpressure + classes == [activity].
        let mut g = RunEventGap::activity("r-1", 4, 7);
        g.delivery = DeliveryMode::Lossless;
        assert!(validate_gap(&g).is_err(), "lossless gap must reject");

        let mut g = RunEventGap::activity("r-1", 4, 7);
        g.classes = vec![RunEventDeliveryClass::Critical];
        assert!(validate_gap(&g).is_err(), "critical class must reject");

        let mut g = RunEventGap::activity("r-1", 4, 7);
        g.classes = vec![
            RunEventDeliveryClass::Activity,
            RunEventDeliveryClass::Normal,
        ];
        assert!(validate_gap(&g).is_err(), "mixed classes must reject");

        // an overflowing range is an Err, never a panic/wrap (checked arithmetic).
        let g = RunEventGap {
            schema_version: crate::schema::run_event_gap_version(),
            run_id: "r-1".into(),
            from_seq: 0,
            to_seq: u64::MAX,
            count: 0,
            reason: GapReason::ActivityBackpressure,
            delivery: DeliveryMode::Balanced,
            classes: vec![RunEventDeliveryClass::Activity],
        };
        assert!(validate_gap(&g).is_err(), "overflow range must reject");
    }
}
