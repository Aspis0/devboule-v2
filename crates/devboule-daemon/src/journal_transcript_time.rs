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
