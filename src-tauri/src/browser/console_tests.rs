use super::*;
use serde_json::json;

/// One event, in the shape the protocol writes it. The three are taken from
/// the CDP documentation's own parameter objects rather than invented here: a
/// parser tested against a shape nothing ever sends is a parser tested against
/// itself.
fn console_call(kind: &str, text: &str, timestamp: f64) -> String {
    json!({
        "type": kind,
        "args": [{ "type": "string", "value": text }],
        "executionContextId": 3,
        "timestamp": timestamp,
        "stackTrace": { "callFrames": [] },
        "context": "top",
    })
    .to_string()
}

fn exception(description: &str, timestamp: f64) -> String {
    json!({
        "exceptionDetails": {
            "exceptionId": 1,
            "text": "Uncaught",
            "lineNumber": 12,
            "columnNumber": 3,
            "exception": {
                "type": "TypeError",
                "subtype": null,
                "className": "TypeError",
                "description": description,
                "objectId": "{\"injectedId\":1}",
            },
            "stackTrace": { "callFrames": [] },
        },
        "timestamp": timestamp,
    })
    .to_string()
}

fn log_entry(level: &str, text: &str, timestamp: f64) -> String {
    json!({
        "entry": {
            "source": "network",
            "level": level,
            "text": text,
            "timestamp": timestamp,
            "url": "https://example.test/app.js",
            "networkRequestId": "1234.5",
        },
        "timestamp": timestamp,
    })
    .to_string()
}

#[test]
fn the_three_events_are_read_and_the_rest_are_not() {
    open("tab-voice");
    record(
        "tab-voice",
        "Runtime.consoleAPICalled",
        &console_call("log", "starting", 1.0),
    );
    record(
        "tab-voice",
        "Runtime.consoleAPICalled",
        &console_call("warning", "slow", 2.0),
    );
    record(
        "tab-voice",
        "Runtime.consoleAPICalled",
        &console_call("error", "no such thing", 3.0),
    );
    record(
        "tab-voice",
        "Runtime.exceptionThrown",
        &exception("TypeError: x is not a function", 4.0),
    );
    record(
        "tab-voice",
        "Log.entryAdded",
        &log_entry("error", "Failed to load resource", 5.0),
    );
    // A page event reaches the same listener and is not the page's voice.
    record(
        "tab-voice",
        "Page.frameNavigated",
        &json!({ "frame": { "id": "1" } }).to_string(),
    );
    // So is a shape nothing here can read.
    record("tab-voice", "Runtime.consoleAPICalled", "{not json");

    let (all, dropped) = entries("tab-voice", Wanted::All, None);
    assert_eq!(dropped, 0);
    assert_eq!(
        all.iter()
            .map(|entry| entry.text.as_str())
            .collect::<Vec<_>>(),
        [
            "starting",
            "slow",
            "no such thing",
            "TypeError: x is not a function",
            "Failed to load resource",
        ]
    );
    assert_eq!(all[3].source.as_deref(), Some("exception"));
    assert_eq!(all[4].source.as_deref(), Some("network"));
    assert_eq!(all[0].source, None, "a page logged it, the runtime did not");
    assert_eq!(all[0].time_ms, 1.0, "in the runtime's own clock");
}

#[test]
fn a_warning_is_the_default_because_an_error_is_a_warning_too() {
    open("tab-level");
    for (kind, at) in [("log", 1.0), ("debug", 2.0), ("info", 3.0)] {
        record(
            "tab-level",
            "Runtime.consoleAPICalled",
            &console_call(kind, "say", at),
        );
    }
    record(
        "tab-level",
        "Runtime.consoleAPICalled",
        &console_call("warning", "careful", 4.0),
    );
    record(
        "tab-level",
        "Runtime.consoleAPICalled",
        &console_call("error", "broken", 5.0),
    );
    record(
        "tab-level",
        "Log.entryAdded",
        &log_entry("info", "cached", 6.0),
    );

    assert_eq!(Wanted::parse(None), Some(Wanted::Warning), "the default");
    assert_eq!(Wanted::parse(Some("all")), Some(Wanted::All));
    assert_eq!(Wanted::parse(Some("loud")), None, "and no other word");

    let (warned, _) = entries("tab-level", Wanted::Warning, None);
    assert_eq!(
        warned.iter().map(|e| e.text.as_str()).collect::<Vec<_>>(),
        ["careful", "broken"]
    );
    let (errors, _) = entries("tab-level", Wanted::Error, None);
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].text, "broken");
    let (everything, _) = entries("tab-level", Wanted::All, None);
    assert_eq!(
        everything.len(),
        6,
        "the runtime's own entries are there too"
    );
}

#[test]
fn one_entry_says_at_most_five_hundred_characters() {
    open("tab-long");
    let said = "x".repeat(900);
    record(
        "tab-long",
        "Runtime.consoleAPICalled",
        &console_call("error", &said, 1.0),
    );

    let (read, _) = entries("tab-long", Wanted::All, None);
    assert_eq!(read[0].text.chars().count(), TEXT_MAX);
}

#[test]
fn the_ring_keeps_the_last_two_hundred_and_says_how_many_it_dropped() {
    open("tab-ring");
    for at in 0..(RING + 5) {
        record(
            "tab-ring",
            "Runtime.consoleAPICalled",
            &console_call("error", &format!("{at}"), at as f64),
        );
    }

    let (read, dropped) = entries("tab-ring", Wanted::Error, None);
    assert_eq!(read.len(), RING);
    assert_eq!(dropped, 5, "and the five that went are counted, not kept");
    assert_eq!(read[0].text, "5", "the oldest five are the ones that went");
    assert_eq!(read[RING - 1].text, format!("{}", RING + 4));
}

#[test]
fn a_new_document_clears_the_ring() {
    open("tab-doc");
    record(
        "tab-doc",
        "Runtime.exceptionThrown",
        &exception("Error: on the way out", 1.0),
    );
    assert_eq!(entries("tab-doc", Wanted::All, None).0.len(), 1);

    clear("tab-doc");
    let (read, dropped) = entries("tab-doc", Wanted::All, None);
    assert!(read.is_empty(), "{read:?}");
    assert_eq!(dropped, 0, "and the count of what was dropped went with it");
}

#[test]
fn since_ms_is_the_last_of_the_page_own_clock() {
    open("tab-since");
    for at in 0..10 {
        record(
            "tab-since",
            "Runtime.consoleAPICalled",
            &console_call("error", &format!("{at}"), at as f64 * 100.0),
        );
    }

    let (recent, _) = entries("tab-since", Wanted::All, Some(250.0));
    assert_eq!(
        recent.iter().map(|e| e.text.as_str()).collect::<Vec<_>>(),
        ["7", "8", "9"],
        "the newest entry is at 900, so 250 ms back is 650"
    );
    let (all, _) = entries("tab-since", Wanted::All, None);
    assert_eq!(all.len(), 10, "and without one nothing is left out");
}

#[test]
fn a_tab_that_was_never_watched_answers_with_nothing() {
    assert_eq!(
        entries("tab-never-watched", Wanted::All, None),
        (Vec::new(), 0)
    );
}

#[test]
fn an_argument_with_no_text_prints_as_the_console_prints_it() {
    open("tab-args");
    record(
        "tab-args",
        "Runtime.consoleAPICalled",
        &json!({
            "type": "log",
            "args": [
                { "type": "object", "className": "Window", "description": "Window" },
                { "type": "object", "className": "Array", "description": "Array(3)" },
                { "type": "number", "value": 42 },
            ],
            "timestamp": 1.0,
        })
        .to_string(),
    );

    let (read, _) = entries("tab-args", Wanted::All, None);
    assert_eq!(read[0].text, "Window Array(3) 42");
}
