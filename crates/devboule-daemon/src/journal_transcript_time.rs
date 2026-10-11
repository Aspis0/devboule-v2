use devboule_protocol::{SessionEvent, UserMessageKind};

/// A kind-less native row predates `message_kind`: the daemon itself wrote
/// it, so its row time is the turn's time. A provider envelope keeps the
/// `Unknown` its payload decodes with — no turn time; the live ACP client
/// drops that echo before journaling or publishing it, so there is no live
/// time to agree with. Callers pass only a native `agent_report` row's
/// decode: the row kind, not the event, is what tells the two apart.
pub(crate) fn time_a_kindless_report(event: &mut SessionEvent, ts_ms: u64) {
    if let SessionEvent::AgentUserMessage {
        at_ms,
        message_kind,
        ..
    } = event
    {
        if at_ms.is_none() && *message_kind == UserMessageKind::Unknown {
            *at_ms = Some(ts_ms);
        }
    }
}

/// A finish carries its time: the row's `ts_ms` on replay, the publish instant
/// live. A finish that already has a time keeps it, so a value the provider
/// view set is never overwritten by the row it came from.
pub(crate) fn time_a_finish(event: &mut SessionEvent, ts_ms: u64) {
    if let SessionEvent::AgentFinished { at_ms, .. } = event {
        if at_ms.is_none() {
            *at_ms = Some(ts_ms);
        }
    }
}

#[cfg(test)]
mod finish_time_tests {
    use super::*;

    fn finished(at_ms: Option<u64>) -> SessionEvent {
        SessionEvent::AgentFinished {
            stop_reason: "end_turn".to_string(),
            model_id: None,
            usage: None,
            at_ms,
        }
    }

    #[test]
    fn a_finish_without_a_time_takes_its_row_time() {
        let mut event = finished(None);
        time_a_finish(&mut event, 1_789_000_217_000);
        assert!(matches!(
            event,
            SessionEvent::AgentFinished {
                at_ms: Some(1_789_000_217_000),
                ..
            }
        ));
    }

    #[test]
    fn a_finish_with_a_time_keeps_it() {
        let mut event = finished(Some(42));
        time_a_finish(&mut event, 99);
        assert!(matches!(
            event,
            SessionEvent::AgentFinished {
                at_ms: Some(42),
                ..
            }
        ));
    }

    #[test]
    fn other_events_are_left_alone() {
        let mut event = SessionEvent::AgentError {
            message: "boom".to_string(),
        };
        time_a_finish(&mut event, 7);
        assert!(matches!(event, SessionEvent::AgentError { .. }));
    }
}
