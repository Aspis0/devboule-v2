//! The client's own behaviour over a scripted transport: redirects judged hop
//! by hop, connections pinned to the checked addresses, one deadline and one
//! cap. The real HTTP transport is exercised by the Oracle forward's tests on
//! the loopback endpoint.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::{send_with, Hop, Limits, Method, OutboundError, Raw, Request, Transport};
use crate::egress_policy::tests::{FakeResolver, CDN, PUBLIC};
use crate::egress_policy::{Resolver, Rule};

/// What the transport saw of one hop.
#[derive(Debug, PartialEq, Eq)]
struct Seen {
    url: String,
    pinned: Option<Vec<String>>,
}

struct Scripted {
    answers: Mutex<VecDeque<Result<Raw, OutboundError>>>,
    seen: Mutex<Vec<Seen>>,
}

impl Scripted {
    fn new(answers: Vec<Result<Raw, OutboundError>>) -> Self {
        Self {
            answers: Mutex::new(answers.into()),
            seen: Mutex::new(Vec::new()),
        }
    }

    fn seen(&self) -> Vec<Seen> {
        std::mem::take(&mut self.seen.lock().expect("seen"))
    }
}

impl Transport for Scripted {
    fn execute(&self, hop: &Hop<'_>) -> Result<Raw, OutboundError> {
        self.seen.lock().expect("seen").push(Seen {
            url: hop.url.to_string(),
            pinned: hop.pin.map(|pin| {
                pin.addresses
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
            }),
        });
        self.answers
            .lock()
            .expect("answers")
            .pop_front()
            .expect("the script has an answer for this hop")
    }
}

fn answer(status: u16, body: &str) -> Result<Raw, OutboundError> {
    Ok(Raw {
        status,
        location: None,
        retry_after: None,
        body: body.as_bytes().to_vec(),
    })
}

fn redirect(to: &str) -> Result<Raw, OutboundError> {
    Ok(Raw {
        status: 302,
        location: Some(to.to_string()),
        retry_after: None,
        body: Vec::new(),
    })
}

fn get(url: &str) -> Request {
    Request {
        method: Method::Get,
        url: url.to_string(),
        headers: Vec::new(),
        body: Vec::new(),
    }
}

fn limits() -> Limits {
    Limits {
        timeout: Duration::from_secs(5),
        max_body: 16,
        max_redirects: 3,
    }
}

fn run(
    transport: &Scripted,
    resolver: &Arc<FakeResolver>,
    rule: &Rule,
    request: &Request,
) -> Result<super::Answer, OutboundError> {
    let resolver: Arc<dyn Resolver> = resolver.clone();
    send_with(transport, &resolver, rule, request, &limits())
}

fn resolver() -> Arc<FakeResolver> {
    FakeResolver::new(&[
        ("cdn.example.test", &[PUBLIC]),
        ("other.example.test", &[PUBLIC]),
        ("rebinds.example.test", &["10.0.0.5"]),
    ])
}

const TWO_HOSTS: Rule = Rule {
    hosts: &["cdn.example.test", "rebinds.example.test"],
    loopback_path: None,
};

#[test]
fn a_public_host_is_asked_at_the_addresses_that_were_checked() {
    let transport = Scripted::new(vec![answer(200, "ok")]);
    let resolver = resolver();
    let reply = run(
        &transport,
        &resolver,
        &CDN,
        &get("https://cdn.example.test/a"),
    )
    .expect("reply");
    assert_eq!(
        (reply.status, reply.body.as_slice()),
        (200, b"ok".as_slice())
    );
    assert_eq!(
        transport.seen(),
        [Seen {
            url: "https://cdn.example.test/a".to_string(),
            pinned: Some(vec![format!("{PUBLIC}:443")]),
        }],
        "the connection goes to the checked address, never a second lookup"
    );
    assert_eq!(resolver.asked(), ["cdn.example.test"]);
}

#[test]
fn redirect_to_private_refused() {
    for target in [
        "https://169.254.169.254/latest/meta-data/",
        "http://cdn.example.test/downgrade",
        "https://other.example.test/elsewhere",
        "https://localhost/",
    ] {
        let transport = Scripted::new(vec![redirect(target), answer(200, "never reached")]);
        let refused = run(
            &transport,
            &resolver(),
            &CDN,
            &get("https://cdn.example.test/a"),
        )
        .expect_err(target);
        assert!(
            matches!(refused, OutboundError::Refused(_)),
            "{target}: {refused:?}"
        );
        assert_eq!(
            transport.seen().len(),
            1,
            "{target}: the second hop is never made"
        );
    }
}

#[test]
fn a_redirect_to_a_name_that_resolves_private_is_refused() {
    let transport = Scripted::new(vec![
        redirect("https://rebinds.example.test/"),
        answer(200, "no"),
    ]);
    let refused = run(
        &transport,
        &resolver(),
        &TWO_HOSTS,
        &get("https://cdn.example.test/"),
    )
    .expect_err("a declared name that now points inside");
    assert!(
        matches!(&refused, OutboundError::Refused(why) if why.contains("private")),
        "{refused:?}"
    );
    assert_eq!(transport.seen().len(), 1);
}

#[test]
fn a_redirect_inside_the_declaration_is_followed_and_each_hop_is_checked() {
    let both = Rule {
        hosts: &["cdn.example.test", "other.example.test"],
        loopback_path: None,
    };
    let transport = Scripted::new(vec![
        redirect("https://other.example.test/x"),
        redirect("/relative"),
        answer(200, "done"),
    ]);
    let resolver = resolver();
    let reply = run(
        &transport,
        &resolver,
        &both,
        &get("https://cdn.example.test/"),
    )
    .expect("reply");
    assert_eq!(reply.body, b"done");
    let urls: Vec<String> = transport.seen().into_iter().map(|seen| seen.url).collect();
    assert_eq!(
        urls,
        [
            "https://cdn.example.test/",
            "https://other.example.test/x",
            "https://other.example.test/relative"
        ]
    );
    assert_eq!(
        resolver.asked(),
        [
            "cdn.example.test",
            "other.example.test",
            "other.example.test"
        ],
        "every hop is resolved and checked again"
    );
}

#[test]
fn redirects_are_bounded_and_a_post_or_loopback_call_follows_none() {
    let looping = Scripted::new(vec![
        redirect("https://cdn.example.test/1"),
        redirect("https://cdn.example.test/2"),
        redirect("https://cdn.example.test/3"),
        redirect("https://cdn.example.test/4"),
    ]);
    let refused = run(
        &looping,
        &resolver(),
        &CDN,
        &get("https://cdn.example.test/0"),
    )
    .expect_err("a loop");
    assert!(matches!(refused, OutboundError::Refused(_)));
    assert_eq!(looping.seen().len(), 4, "the first request and three hops");

    let post = Request {
        method: Method::Post,
        ..get("https://cdn.example.test/")
    };
    let transport = Scripted::new(vec![redirect("https://cdn.example.test/other")]);
    assert!(matches!(
        run(&transport, &resolver(), &CDN, &post),
        Err(OutboundError::Refused(_))
    ));

    let oracle = Rule {
        hosts: &[],
        loopback_path: Some("/oracle/v1/query"),
    };
    let transport = Scripted::new(vec![redirect("https://cdn.example.test/")]);
    let loopback = Request {
        method: Method::Post,
        ..get("http://127.0.0.1:41234/oracle/v1/query")
    };
    assert!(matches!(
        run(&transport, &resolver(), &oracle, &loopback),
        Err(OutboundError::Refused(_))
    ));
    assert_eq!(
        transport.seen()[0].pinned,
        None,
        "the loopback endpoint is not pinned to a looked-up name"
    );
}

#[test]
fn a_body_past_the_cap_is_too_large_and_a_refused_url_sends_nothing() {
    let transport = Scripted::new(vec![answer(200, &"x".repeat(17))]);
    assert_eq!(
        run(
            &transport,
            &resolver(),
            &CDN,
            &get("https://cdn.example.test/")
        )
        .err(),
        Some(OutboundError::TooLarge)
    );
    let transport = Scripted::new(vec![]);
    assert!(matches!(
        run(&transport, &resolver(), &CDN, &get("https://127.0.0.1/")),
        Err(OutboundError::Refused(_))
    ));
    assert!(transport.seen().is_empty());
}

#[test]
fn the_transports_own_failures_pass_through_unchanged() {
    for failure in [
        OutboundError::Timeout,
        OutboundError::Cut,
        OutboundError::Transport,
    ] {
        let expected = format!("{failure:?}");
        let transport = Scripted::new(vec![Err(failure)]);
        let got = run(
            &transport,
            &resolver(),
            &CDN,
            &get("https://cdn.example.test/"),
        )
        .err()
        .map(|error| format!("{error:?}"));
        assert_eq!(got, Some(expected));
    }
}
