//! `act`: a sequence of commands an agent already knows, run as one.
//!
//! The steps are the single-action commands and nothing else: they run through
//! the same code, on the same page, with the same settling, so a batch cannot
//! mean anything a step on its own does not. What a batch adds is one deadline
//! and one answer — a page an agent has to steer through four moves gets one
//! delta for the four, not four deltas it has to read and merge.
//!
//! Everything is checked before anything runs. A step naming a command this
//! host does not run, or a step that is a batch itself, is refused whole: half
//! a batch is worse than none, because the caller cannot tell which half
//! happened.
//!
//! The steps themselves are not checked: a `fill` with no ref is a step that
//! fails, and the batch answers with the steps that ran, the one that did not,
//! and the delta in between.

use serde::Deserialize;
use serde_json::{json, Map, Value};

use devboule_protocol::BrowserError;

use super::super::cdp::Page;
use super::super::commands::tabs;
use super::super::deadline::Deadline;
use super::super::delta;
use super::super::registry::TabInfo;
use super::{act, args_of, code_name, host_error, input, wait};

/// The commands a batch may carry. `act` is not one of them: a batch that
/// could nest is a deadline nobody can reason about.
pub const STEPS: [&str; 10] = [
    "click", "fill", "type", "press", "select", "check", "hover", "scroll", "wait_for", "navigate",
];

/// The most steps one batch may carry, the contract's own cap.
pub const MAX_STEPS: usize = 10;

#[derive(Deserialize)]
struct Batch {
    steps: Vec<Step>,
}

#[derive(Deserialize)]
struct Step {
    command: String,
    #[serde(flatten)]
    rest: Map<String, Value>,
}

pub async fn run(
    tab: &TabInfo,
    page: &dyn Page,
    args: &Value,
    deadline: Deadline,
) -> Result<Value, BrowserError> {
    let asked: Batch = args_of(args)?;
    checked(&asked.steps)?;
    // One read before the first step and one after the last: the delta is what
    // the whole batch did to the page, which is the only thing a caller with
    // ten steps wants to read.
    let start = act::read(tab, page).await?;
    let mut steps = Vec::new();
    for step in &asked.steps {
        match one(tab, page, step, deadline).await {
            Ok(_) => steps.push(json!({ "command": step.command, "ok": true })),
            Err(error) => {
                steps.push(json!({
                    "command": step.command,
                    "ok": false,
                    "error": { "code": code_name(error.code), "message": error.message },
                }));
                break;
            }
        }
    }
    let after = act::settled(tab, page, deadline).await?;
    let delta = delta::between(&start.view, &after, &start.place, &act::place(tab), None);
    Ok(json!({ "steps": steps, "delta": delta }))
}

/// What every step must be before the first one runs.
fn checked(steps: &[Step]) -> Result<(), BrowserError> {
    if steps.is_empty() || steps.len() > MAX_STEPS {
        return Err(host_error(format!(
            "act takes from 1 to {MAX_STEPS} steps; this one has {}.",
            steps.len()
        )));
    }
    for step in steps {
        if step.command == "act" {
            return Err(host_error(
                "an act step cannot be an act: send the steps themselves.",
            ));
        }
        if !STEPS.contains(&step.command.as_str()) {
            return Err(host_error(format!(
                "{} is not a command act can run. It runs {}.",
                step.command,
                STEPS.join(", ")
            )));
        }
    }
    Ok(())
}

/// One step, through the same command the caller could have sent on its own.
async fn one(
    tab: &TabInfo,
    page: &dyn Page,
    step: &Step,
    deadline: Deadline,
) -> Result<Value, BrowserError> {
    let args = Value::Object(step.rest.clone());
    match step.command.as_str() {
        "navigate" => tabs::navigate(tab, page, &args, deadline).await,
        "wait_for" => wait::wait_for(tab, page, &args, deadline).await,
        command => input::run(tab, page, command, &args, deadline).await,
    }
}

#[cfg(test)]
#[path = "batch_tests.rs"]
mod tests;
