//! The two host commands a saved login answers: what may be used on the page
//! in front of the agent, and the typing itself.
//!
//! `fill_login_preview` answers what a person may choose from — the origin of
//! the frame the field lives in, and the labels of the logins that site allows
//! — and nothing else: no username, no password, and nothing an agent could
//! read a list of entries out of. `fill_login` types the entry named by the id
//! the person chose on the card, and answers a constant: which of the
//! arguments it filled, never a view and never a delta.
//!
//! The gate is the origin of the field's OWN frame, not the tab's address: a
//! field inside a hostile cross-origin frame is the case this exists for. It
//! is read out of the vault's own list of what a site may use, so a login that
//! site does not allow is refused before the OS store is read at all — while
//! the site is wrong there is no password in this process to leak.
//!
//! The page can change under a field between the checks and the typing, and no
//! check can be made atomic with the typing. What is left is to do the work
//! that must not happen (focus, clear) first, read the password at the last
//! moment, and re-read the field's frame and the page's own frame, and ask the
//! page whether the field holds the focus, immediately before
//! `Input.insertText`. A field that moved or lost the focus is refused and
//! nothing is typed; a document that committed replaces the page's own frame
//! id, which is the only document token a caller can see.

use std::sync::Arc;

use serde::Deserialize;
use serde_json::{json, Value};

use devboule_protocol::{BrowserError, BrowserErrorCode};

use crate::backend::blocking::off_main_thread;

use super::super::cdp::Page;
use super::super::commands::page_script;
use super::super::commands::{act, args_of, host_error, node_of};
use super::super::registry::TabInfo;
use super::super::scrub;
use super::field_check::{self, Kind};
use super::field_frame;
use super::origin::{self, Origin};
use super::Vault;

/// The arguments of both commands. At least one ref: a login page is filled a
/// field at a time, and a call with no field has nothing to type into.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Refs {
    #[serde(rename = "usernameRef")]
    username: Option<String>,
    #[serde(rename = "passwordRef")]
    password: Option<String>,
    /// Which login to type. The daemon adds it once the person has chosen on
    /// the card; an agent cannot send one, because the tool's own argument set
    /// is closed and the daemon refuses a name it does not offer.
    entry_id: Option<String>,
}

/// One field to type into: the node, and the frame it was in when it was read.
struct Field {
    reference: String,
    node: u64,
    frame: String,
}

/// Everything one call is about: one origin, one document, and the fields.
struct Target {
    origin: Origin,
    /// The page's own frame id, which a new document replaces.
    document: String,
    username: Option<Field>,
    password: Option<Field>,
}

pub const COMMANDS: [&str; 2] = ["fill_login_preview", "fill_login"];

/// Run the command the daemon named, over this machine's own saved logins.
pub async fn run(
    app: &tauri::AppHandle,
    tab: &TabInfo,
    page: &dyn Page,
    command: &str,
    args: &Value,
) -> Result<Value, BrowserError> {
    let dir = super::commands::local_folder(app).map_err(|error| host_error(error.message))?;
    let vault = Arc::new(Vault::in_dir(dir));
    match command {
        "fill_login_preview" => preview(&vault, page, args).await,
        _ => fill(&vault, tab, page, args).await,
    }
}

/// The canonical origin of the tab's own frame right now, in the form the
/// vault stores and compares. `None` when its address is one no site can be
/// compared on.
///
/// The scrub bounds a typed password by this: it is kept while the tab's own
/// frame is on the site the tab was on when it was typed, and dropped when the
/// tab is elsewhere. The field's frame can be another site's, so it is the
/// tab's address and not the field's that stands for "the page".
pub fn origin_of(tab: &TabInfo) -> Option<String> {
    let url = tab
        .state
        .lock()
        .expect("browser state poisoned")
        .url
        .clone();
    origin::of_page(&url)
        .ok()
        .map(|origin| origin.as_str().to_owned())
}

/// What the person may choose from: the site, and the labels of the logins it
/// allows. Nothing here is a credential, and no entry is named by anything the
/// agent chose.
async fn preview(vault: &Vault, page: &dyn Page, args: &Value) -> Result<Value, BrowserError> {
    let asked: Refs = args_of(args)?;
    settled(page).await?;
    let target = resolve(page, &asked).await?;
    let entries = vault
        .lookup_for_origin(&target.origin)
        .map_err(|refusal| host_error(refusal.sentence()))?;
    if entries.is_empty() {
        return Err(host_error(format!(
            "no_saved_login: this page's own site is {}, and this machine has no saved \
             login for it.",
            target.origin
        )));
    }
    let entries = entries
        .iter()
        .map(|entry| json!({ "id": entry.id, "label": entry.label }))
        .collect::<Vec<_>>();
    Ok(json!({
        "origin": target.origin.as_str(),
        "entries": entries,
    }))
}

/// Type one saved login into the fields the call named.
async fn fill(
    vault: &Arc<Vault>,
    tab: &TabInfo,
    page: &dyn Page,
    args: &Value,
) -> Result<Value, BrowserError> {
    let asked: Refs = args_of(args)?;
    let Some(entry_id) = asked.entry_id.as_deref() else {
        return Err(host_error(
            "This command needs the id of the saved login to type.",
        ));
    };
    settled(page).await?;
    let target = resolve(page, &asked).await?;
    let entry = vault
        .lookup_for_origin(&target.origin)
        .map_err(|refusal| host_error(refusal.sentence()))?
        .into_iter()
        .find(|entry| entry.id == entry_id)
        .ok_or_else(|| {
            host_error(format!(
                "origin_mismatch: that saved login is not one of this page's own site's \
                 ({}).",
                target.origin
            ))
        })?;
    let mut filled: Vec<&str> = Vec::new();
    if let Some(field) = &target.username {
        prepared(page, field).await?;
        typed_into(page, &target, field, &entry.username, None).await?;
        filled.push("usernameRef");
    }
    if let Some(field) = &target.password {
        prepared(page, field).await?;
        // The store is read here and nowhere earlier: every check that could
        // refuse has refused, and the next things this process does with the
        // string are to look at the page once more and type it.
        let password = password_of(vault, &entry.id).await?;
        let page_origin = origin_of(tab).unwrap_or_default();
        let held = Held {
            tab: &tab.browser_id,
            page_origin: &page_origin,
        };
        typed_into(page, &target, field, &password, Some(held)).await?;
        filled.push("passwordRef");
    }
    Ok(json!({ "filled": filled }))
}

/// The one origin every ref of this call lives on, and the fields themselves.
///
/// One call is one site: the card can name one origin and one entry, and two
/// fields of two sites have no single answer to be approved against.
async fn resolve(page: &dyn Page, asked: &Refs) -> Result<Target, BrowserError> {
    let named: [(bool, &str); 2] = [
        (true, asked.username.as_deref().unwrap_or_default()),
        (false, asked.password.as_deref().unwrap_or_default()),
    ];
    if named.iter().all(|(_, reference)| reference.is_empty()) {
        return Err(host_error(
            "This command needs a usernameRef or a passwordRef to fill.",
        ));
    }
    let tree = act::call(page, "Page.getFrameTree", json!({})).await?;
    let document = tree
        .pointer("/frameTree/frame/id")
        .and_then(Value::as_str)
        .ok_or_else(|| host_error("This page reports no frame of its own."))?
        .to_owned();
    let every = field_frame::frames_of(&tree)?;
    let mut found: Vec<(bool, Field, Origin)> = Vec::new();
    for (is_username, reference) in named {
        if reference.is_empty() {
            continue;
        }
        let node = node_of(reference)?;
        let frame = field_frame::frame_of(page, node, &every).await?;
        let origin = field_frame::origin_in(&every, &frame)?;
        let kind = if is_username {
            Kind::Username
        } else {
            Kind::Password
        };
        field_check::fillable(page, node, kind).await?;
        if let Some((_, _, here)) = found.first() {
            if *here != origin {
                return Err(host_error(format!(
                    "Those fields are on two different sites ({} and {}); one saved login is \
                     typed on one site.",
                    here, origin
                )));
            }
        }
        found.push((
            is_username,
            Field {
                reference: reference.to_owned(),
                node,
                frame,
            },
            origin,
        ));
    }
    let mut target = Target {
        origin: found
            .first()
            .map(|(_, _, origin)| origin.clone())
            .expect("a named ref was found"),
        document,
        username: None,
        password: None,
    };
    for (is_username, field, _) in found {
        if is_username {
            target.username = Some(field);
        } else {
            target.password = Some(field);
        }
    }
    Ok(target)
}

/// A page that is still loading is not one this app types a password into: its
/// frames are mid-replacement, and every check below is about a page that has
/// stopped moving.
async fn settled(page: &dyn Page) -> Result<(), BrowserError> {
    let state = page_script::evaluated(page, "document.readyState").await?;
    if state.as_str() != Some("complete") {
        return Err(host_error(
            "This page is still loading; wait for it and try again.",
        ));
    }
    Ok(())
}

/// What the page is asked before any value may go in: bring the field into
/// view, focus it, and empty it the way a person's typing empties it. A field
/// that cannot be emptied is not one this app types into.
async fn prepared(page: &dyn Page, field: &Field) -> Result<(), BrowserError> {
    act::into_view(page, field.node).await?;
    act::call(page, "DOM.focus", json!({ "backendNodeId": field.node })).await?;
    let cleared = page_script::on_node(page, field.node, page_script::CLEAR, json!([])).await?;
    if cleared != Value::Bool(true) {
        return Err(host_error(
            "That field could not be emptied, so nothing was typed; take a new snapshot and              look again.",
        ));
    }
    Ok(())
}

/// The same field, in the same frame, in the same document, as when it was
/// read. This is what stands between a checked field and whatever the page has
/// put in its place.
async fn unchanged(page: &dyn Page, target: &Target, field: &Field) -> Result<(), BrowserError> {
    let moved = || {
        host_error(
            "That field moved, or the page changed under it, before the value could be typed; \
             take a new snapshot and look again.",
        )
    };
    let tree = act::call(page, "Page.getFrameTree", json!({})).await?;
    if tree.pointer("/frameTree/frame/id").and_then(Value::as_str) != Some(&target.document) {
        return Err(moved());
    }
    // A field that is simply gone is reported as the change it is: this check
    // asks precisely whether the page moved, and the advice a stale ref gives
    // is the advice this one wants to give.
    let frame = field_frame::frame_of(page, field.node, &field_frame::frames_of(&tree)?)
        .await
        .map_err(|_| moved())?;
    if frame != field.frame || node_of(&field.reference)? != field.node {
        return Err(moved());
    }
    Ok(())
}

/// Where a typed secret is remembered for the scrub: the tab, and the site that
/// tab was on.
struct Held<'a> {
    tab: &'a str,
    page_origin: &'a str,
}

/// The last look at the field, and then the value, as inserted text: what a
/// person's typing arrives as and what a framework's value tracker sees.
///
/// A secret is registered with the scrub right before it is sent and no await
/// stands between the two, so nothing the page does with it can be answered
/// before it is covered. A send the page refused took nothing in and the
/// registration is withdrawn; one that ran out of time may have, and keeps it.
async fn typed_into(
    page: &dyn Page,
    target: &Target,
    field: &Field,
    text: &str,
    held: Option<Held<'_>>,
) -> Result<(), BrowserError> {
    unchanged(page, target, field).await?;
    field_check::focused(page, field.node).await?;
    let added = held
        .as_ref()
        .is_some_and(|held| scrub::remember(held.tab, held.page_origin, text));
    let sent = act::call(page, "Input.insertText", json!({ "text": text })).await;
    if let (Err(error), Some(held), true) = (&sent, &held, added) {
        if error.code != BrowserErrorCode::Timeout {
            scrub::withdraw(held.tab, held.page_origin, text);
        }
    }
    sent.map(|_| ())
}

/// The password of one entry, read off the thread that serves other commands:
/// an OS credential store can wait on a person.
async fn password_of(vault: &Arc<Vault>, id: &str) -> Result<String, BrowserError> {
    let (vault, id) = (Arc::clone(vault), id.to_owned());
    off_main_thread(move || vault.password_for(&id))
        .await
        .map_err(|error| host_error(error.message))?
        .ok_or_else(|| {
            host_error("This machine's credential store holds no password for that saved login.")
        })
}

#[cfg(test)]
#[path = "fill_login_fixtures.rs"]
mod fixtures;
#[cfg(test)]
#[path = "fill_login_target_tests.rs"]
mod target_tests;
#[cfg(test)]
#[path = "fill_login_tests.rs"]
mod tests;
