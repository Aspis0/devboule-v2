//! `act`: a sequence of commands an agent already knows, run as one.
//!
//! The steps are the single-action commands and nothing else: they run through
//! the same code, on the same page, with the same settling, so a batch cannot
//! mean anything a step on its own does not. What a batch adds is one deadline
//! and one answer — a page an agent has to steer through four moves gets one
//! delta for the four, not four deltas it has to read and merge.
//!
//! Everything is checked before anything runs. A step naming a command this
//! host does not run, a step that is a batch itself, and a step whose own
//! arguments are not the ones its command takes: all of them are refused whole,
//! because half a batch is worse than none — the caller cannot tell which half
//! happened.
//!
//! What a batch cannot undo is what a step did. A page an agent typed into has
//! already changed by the time a deadline runs out, so a batch that runs out of
//! time answers with the steps that ran and `settled: false`, never with a bare
//! refusal that reads as "nothing happened".

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
                steps.push(failed(&step.command, &error));
                break;
            }
        }
    }
    // The settle is what the deadline is usually spent on, and it is what used
    // to throw the steps away: the page may well have changed, so the answer is
    // the steps and no delta rather than a refusal that reads as nothing done.
    let after = act::settled(tab, page, deadline).await.ok();
    let delta = after
        .as_ref()
        .map(|after| delta::between(&start.view, after, &start.place, &act::place(tab), None));
    Ok(json!({
        "steps": steps,
        "delta": delta,
        // Whether the page settled inside the budget. A caller that needs the
        // delta it did not get can take another look and ask again.
        "settled": after.is_some(),
    }))
}

/// One step's own outcome, in the protocol's words: a budget that ran out is a
/// timeout and not something the page refused.
fn failed(command: &str, error: &BrowserError) -> Value {
    json!({
        "command": command,
        "ok": false,
        "error": { "code": code_name(error.code), "message": error.message },
    })
}

/// What every step must be before the first one runs: a command this host runs,
/// and arguments that command's own shape accepts.
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
        // The step's own arguments, read by the command that will run it: a
        // batch that cannot run whole is refused whole, rather than doing its
        // first step and failing on the second.
        command_args(&step.command, &step.rest)?;
    }
    Ok(())
}

/// One step's arguments, checked against the shape the command reads. The
/// commands that answer something rather than acting — a navigation, a wait —
/// are checked by their own readers, so a batch refuses what they would.
fn command_args(command: &str, rest: &Map<String, Value>) -> Result<(), BrowserError> {
    let args = Value::Object(rest.clone());
    match command {
        "navigate" => {
            let _: tabs::NavigateArgs = args_of(&args)?;
        }
        "wait_for" => {
            wait::checked(&args)?;
        }
        other => input::check_args(other, &args)?,
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
