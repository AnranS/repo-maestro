//! F-120 pure delivery planner.
//!
//! Given the durable events + a cursor + a delivery mode, produce the ordered live
//! frames (a concrete event, or a shed-activity gap) WITHOUT touching the
//! filesystem, the ledger, or the network. `lossless` is byte-identical to the
//! F-115 `event_stream_frames` (every event with `seq > cursor`). `balanced` keeps
//! every critical/normal event plus the most recent activity tail, and covers older
//! contiguous activity-only spans with gap frames — a critical OR normal event
//! NEVER appears inside a gap range, and the durable ledger keeps every class.

use crate::scheduler::events::{RunEvent, RunEventKind};
use crate::schema::event_delivery::{DeliveryMode, RunEventDeliveryClass, RunEventGap};

/// Balanced-mode tuning (initial values; tunable after dogfood).
pub const BALANCED_SOFT_FRAME_CAP: usize = 256;
pub const BALANCED_ACTIVITY_KEEP_TAIL: usize = 64;

/// Balanced-mode shedding knobs; defaults to the constants above.
#[derive(Debug, Clone, Copy)]
pub struct DeliveryOptions {
    /// Below this many pending events, balanced emits everything concretely.
    pub soft_frame_cap: usize,
    /// Newest activity events kept concrete before older activity is gap-shed.
    pub activity_keep_tail: usize,
}

impl Default for DeliveryOptions {
    fn default() -> Self {
        Self {
            soft_frame_cap: BALANCED_SOFT_FRAME_CAP,
            activity_keep_tail: BALANCED_ACTIVITY_KEEP_TAIL,
        }
    }
}

/// One planned live frame.
#[derive(Debug, Clone, PartialEq)]
pub enum DeliveryFrame {
    Event(RunEvent),
    Gap(RunEventGap),
}

impl DeliveryFrame {
    /// The seq this frame advances the consumer cursor to (an event's `seq`, a
    /// gap's `to_seq`).
    pub fn cursor_seq(&self) -> u64 {
        match self {
            DeliveryFrame::Event(e) => e.seq,
            DeliveryFrame::Gap(g) => g.to_seq,
        }
    }
}

/// The live-delivery class of an event. `Other(_)` (an unknown future wire kind) is
/// `critical` by forward-safety — a kind we do not understand is never shed.
pub fn classify_event(event: &RunEvent) -> RunEventDeliveryClass {
    use RunEventDeliveryClass::*;
    use RunEventKind::*;
    match &event.kind {
        RunCompleted
        | RunFailed
        | RunCancelled
        | TaskSucceeded
        | TaskFailed
        | TaskCancelled
        | TaskSkipped
        | TaskApprovalRequested
        | TaskApprovalGranted
        | CancelRequested
        | FindingRecorded
        | Other(_) => Critical,
        RunCreated | TaskQueued | TaskStarted | ReplanWritten => Normal,
        VerifyStarted | VerifyCompleted => Activity,
    }
}

/// Plan the ordered live frames for `events` past `cursor` under `mode`.
pub fn plan_delivery_frames(
    events: &[RunEvent],
    cursor: u64,
    mode: DeliveryMode,
    opts: DeliveryOptions,
) -> Vec<DeliveryFrame> {
    // seq > cursor, in seq order (mirrors event_stream_frames / RunEventStream).
    let mut pending: Vec<&RunEvent> = events.iter().filter(|e| e.seq > cursor).collect();
    pending.sort_by_key(|e| e.seq);

    // lossless, or below the soft cap → every event is concrete (no shedding).
    if mode == DeliveryMode::Lossless || pending.len() <= opts.soft_frame_cap {
        return pending
            .into_iter()
            .cloned()
            .map(DeliveryFrame::Event)
            .collect();
    }

    // Above the cap: shed OLDER activity events, keeping the most recent tail concrete.
    let mut activity_seqs: Vec<u64> = pending
        .iter()
        .filter(|e| classify_event(e) == RunEventDeliveryClass::Activity)
        .map(|e| e.seq)
        .collect();
    activity_seqs.sort_unstable();
    let keep_from = activity_seqs.len().saturating_sub(opts.activity_keep_tail);
    let kept_activity: std::collections::BTreeSet<u64> =
        activity_seqs[keep_from..].iter().copied().collect();

    let is_shed = |e: &RunEvent| {
        classify_event(e) == RunEventDeliveryClass::Activity && !kept_activity.contains(&e.seq)
    };

    // Walk in seq order. A gap merges ONLY truly-consecutive shed seqs
    // (`e.seq == prev_to + 1`); a non-consecutive shed event flushes the open gap and
    // starts a new one, so a gap never claims to cover a seq it did not see (a missing
    // seq between two shed activity events stays uncovered). A kept event
    // (critical/normal/kept-activity) flushes the gap and emits concretely, so a gap
    // never spans a kept seq either.
    let mut frames: Vec<DeliveryFrame> = Vec::new();
    let mut gap_run: Option<(u64, u64, String)> = None; // (from, to, run_id)
    for e in pending {
        if is_shed(e) {
            let extends =
                matches!(gap_run.as_ref(), Some((_, to, _)) if to.checked_add(1) == Some(e.seq));
            if extends {
                if let Some((_, to, _)) = gap_run.as_mut() {
                    *to = e.seq;
                }
            } else {
                if let Some((from, to, run_id)) = gap_run.take() {
                    frames.push(DeliveryFrame::Gap(RunEventGap::activity(run_id, from, to)));
                }
                gap_run = Some((e.seq, e.seq, e.run_id.clone()));
            }
        } else {
            if let Some((from, to, run_id)) = gap_run.take() {
                frames.push(DeliveryFrame::Gap(RunEventGap::activity(run_id, from, to)));
            }
            frames.push(DeliveryFrame::Event(e.clone()));
        }
    }
    if let Some((from, to, run_id)) = gap_run.take() {
        frames.push(DeliveryFrame::Gap(RunEventGap::activity(run_id, from, to)));
    }
    frames
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::event_delivery::validate_gap;

    fn ev(seq: u64, kind: RunEventKind) -> RunEvent {
        RunEvent {
            schema_version: crate::schema::run_event_version(),
            event_id: format!("r-1-{seq}"),
            run_id: "r-1".into(),
            seq,
            timestamp: "2026-06-05T00:00:00Z".parse().unwrap(),
            kind,
            task_id: None,
            status: None,
            severity: None,
            message: None,
            display: None,
            payload: serde_json::Value::Null,
            refs: Default::default(),
        }
    }

    fn small() -> DeliveryOptions {
        DeliveryOptions {
            soft_frame_cap: 2,
            activity_keep_tail: 1,
        }
    }

    #[test]
    fn classify_maps_every_class() {
        assert_eq!(
            classify_event(&ev(1, RunEventKind::RunFailed)),
            RunEventDeliveryClass::Critical
        );
        assert_eq!(
            classify_event(&ev(1, RunEventKind::TaskSkipped)),
            RunEventDeliveryClass::Critical
        );
        assert_eq!(
            classify_event(&ev(1, RunEventKind::TaskApprovalRequested)),
            RunEventDeliveryClass::Critical
        );
        assert_eq!(
            classify_event(&ev(1, RunEventKind::FindingRecorded)),
            RunEventDeliveryClass::Critical
        );
        // an unknown future kind is critical (forward-safe), never shed
        assert_eq!(
            classify_event(&ev(1, RunEventKind::Other("usage.sampled".into()))),
            RunEventDeliveryClass::Critical
        );
        // ReplanWritten (wire evidence.captured) is normal in v1, not shed
        assert_eq!(
            classify_event(&ev(1, RunEventKind::ReplanWritten)),
            RunEventDeliveryClass::Normal
        );
        assert_eq!(
            classify_event(&ev(1, RunEventKind::TaskStarted)),
            RunEventDeliveryClass::Normal
        );
        assert_eq!(
            classify_event(&ev(1, RunEventKind::VerifyStarted)),
            RunEventDeliveryClass::Activity
        );
        assert_eq!(
            classify_event(&ev(1, RunEventKind::VerifyCompleted)),
            RunEventDeliveryClass::Activity
        );
    }

    #[test]
    fn lossless_emits_every_event_past_cursor() {
        let events = vec![
            ev(1, RunEventKind::VerifyStarted),
            ev(2, RunEventKind::VerifyCompleted),
            ev(3, RunEventKind::RunCompleted),
        ];
        let frames = plan_delivery_frames(&events, 1, DeliveryMode::Lossless, small());
        // seq > 1, all concrete (no gaps) even though cap is small
        assert_eq!(frames.len(), 2);
        assert!(frames.iter().all(|f| matches!(f, DeliveryFrame::Event(_))));
        assert_eq!(frames[0].cursor_seq(), 2);
        assert_eq!(frames[1].cursor_seq(), 3);
    }

    #[test]
    fn balanced_below_cap_emits_everything() {
        let events = vec![
            ev(1, RunEventKind::VerifyStarted),
            ev(2, RunEventKind::VerifyCompleted),
        ];
        let frames = plan_delivery_frames(
            &events,
            0,
            DeliveryMode::Balanced,
            DeliveryOptions {
                soft_frame_cap: 10,
                activity_keep_tail: 1,
            },
        );
        assert_eq!(frames.len(), 2);
        assert!(frames.iter().all(|f| matches!(f, DeliveryFrame::Event(_))));
    }

    #[test]
    fn balanced_gaps_old_activity_keeps_tail_and_validates() {
        let events = vec![
            ev(1, RunEventKind::VerifyStarted),
            ev(2, RunEventKind::VerifyCompleted),
            ev(3, RunEventKind::VerifyStarted),
            ev(4, RunEventKind::VerifyCompleted), // newest activity → kept (tail=1)
            ev(5, RunEventKind::RunCompleted),    // critical
        ];
        let frames = plan_delivery_frames(&events, 0, DeliveryMode::Balanced, small());
        // expect: Gap[1..3], Event(4), Event(5)
        assert_eq!(frames.len(), 3);
        match &frames[0] {
            DeliveryFrame::Gap(g) => {
                assert_eq!((g.from_seq, g.to_seq, g.count), (1, 3, 3));
                assert!(validate_gap(g).is_ok());
            }
            other => panic!("expected gap, got {other:?}"),
        }
        assert_eq!(
            frames[1],
            DeliveryFrame::Event(ev(4, RunEventKind::VerifyCompleted))
        );
        // the critical event is concrete, never inside a gap
        assert!(matches!(&frames[2], DeliveryFrame::Event(e) if e.seq == 5));
    }

    #[test]
    fn balanced_splits_gap_around_a_critical_event() {
        let events = vec![
            ev(1, RunEventKind::VerifyStarted),
            ev(2, RunEventKind::RunFailed), // critical in the middle
            ev(3, RunEventKind::VerifyStarted),
            ev(4, RunEventKind::VerifyCompleted),
            ev(5, RunEventKind::VerifyStarted), // newest activity → kept (tail=1)
        ];
        let frames = plan_delivery_frames(&events, 0, DeliveryMode::Balanced, small());
        // expect: Gap[1,1], Event(2 critical), Gap[3,4], Event(5)
        assert_eq!(frames.len(), 4);
        assert!(matches!(&frames[0], DeliveryFrame::Gap(g) if (g.from_seq, g.to_seq) == (1, 1)));
        assert!(matches!(&frames[1], DeliveryFrame::Event(e) if e.seq == 2));
        assert!(
            matches!(&frames[2], DeliveryFrame::Gap(g) if (g.from_seq, g.to_seq, g.count) == (3, 4, 2))
        );
        assert!(matches!(&frames[3], DeliveryFrame::Event(e) if e.seq == 5));
        // no critical event seq is ever covered by a gap range
        for f in &frames {
            if let DeliveryFrame::Gap(g) = f {
                assert!(!(g.from_seq..=g.to_seq).contains(&2));
            }
        }
    }

    #[test]
    fn balanced_gap_never_spans_a_missing_seq() {
        // N2: seq 2 is absent. 1 and 3 are NOT consecutive → separate gaps; 3,4 ARE
        // consecutive → one gap. A gap must never claim to cover the unseen seq 2.
        let events = vec![
            ev(1, RunEventKind::VerifyStarted),
            ev(3, RunEventKind::VerifyStarted),
            ev(4, RunEventKind::VerifyStarted),
        ];
        let opts = DeliveryOptions {
            soft_frame_cap: 2,
            activity_keep_tail: 0,
        };
        let frames = plan_delivery_frames(&events, 0, DeliveryMode::Balanced, opts);
        assert_eq!(frames.len(), 2);
        assert!(
            matches!(&frames[0], DeliveryFrame::Gap(g) if (g.from_seq, g.to_seq, g.count) == (1, 1, 1))
        );
        assert!(
            matches!(&frames[1], DeliveryFrame::Gap(g) if (g.from_seq, g.to_seq, g.count) == (3, 4, 2))
        );
        for f in &frames {
            if let DeliveryFrame::Gap(g) = f {
                assert!(
                    !(g.from_seq..=g.to_seq).contains(&2),
                    "gap must not cover missing seq 2"
                );
            }
        }
    }

    #[test]
    fn frames_advance_cursor_monotonically() {
        let events: Vec<RunEvent> = (1..=6)
            .map(|s| ev(s, RunEventKind::VerifyStarted))
            .collect();
        let frames = plan_delivery_frames(&events, 0, DeliveryMode::Balanced, small());
        let mut last = 0;
        for f in &frames {
            assert!(f.cursor_seq() > last, "cursor must advance");
            last = f.cursor_seq();
        }
    }
}
