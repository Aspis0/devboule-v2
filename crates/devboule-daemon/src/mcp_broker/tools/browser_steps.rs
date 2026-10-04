//! The ten commands one `act` step may run, what a step may carry, and whether
//! a `steps` argument is a batch of them.
//!
//! A step is checked against the row its own command declares — the same rows
//! `tools/list` shows and the daemon refuses against — with the batch's tab
//! standing in for the one that command declares. So there is no second spelling
//! of ten commands' arguments here, neither in what an agent is offered nor in
//! what is refused, and a step cannot offer an argument its own command would
//! reject. The host receives the step exactly as the agent wrote it: the tab is
//! added to the copy that is checked, never to the one that is sent.

use serde_json::{json, Map, Value};

use super::browser_args::{parse, Spec};
use super::browser_commands::spec_of;

/// The commands a batch may run, in the contract's order. `act` is absent, and
/// so is every command that is not one act on one tab: a step naming another
/// batch would have to be checked by a rule of its own.
pub(in crate::mcp_broker) const ACT_COMMANDS: &[&str] = &[
    "click", "fill", "type", "press", "select", "check", "hover", "scroll", "wait_for", "navigate",
];

/// The contract's batch length. A batch over the cap is two turns of work, not
/// one, and the host runs the whole thing inside a single deadline.
pub(in crate::mcp_broker) const MIN_STEPS: usize = 1;
pub(in crate::mcp_broker) const MAX_STEPS: usize = 10;

const TAB: &str = "browserId";

/// The tab a step is checked against. The batch's own `browserId` is checked by
/// its own field and a step naming a tab of its own is refused, so this only has
/// to be an id a `Kind::Tab` accepts: nothing reads it but the row the step is
/// checked against.
const BATCH_TAB: &str = "tab-of-the-batch";

/// What a step may carry, one shape per command: `command` pinned with
/// `const`, and beside it the arguments that command itself takes, read out of
/// the row the daemon refuses against. So a `fill` step is offered `ref` and
/// `text` and not `value`, and an agent never has to guess a step's arguments
/// and then be refused for guessing.
pub(in crate::mcp_broker) fn step_schema() -> Value {
    json!({
        "oneOf": ACT_COMMANDS.iter().filter_map(|command| step_shape(command)).collect::<Vec<_>>(),
    })
}

fn step_shape(command: &str) -> Option<Value> {
    let spec = spec_of(command)?;
    let mut properties = Map::from_iter([(
        "command".to_string(),
        json!({"type": "string", "const": command}),
    )]);
    let mut required = vec!["command"];
    for field in spec.fields.iter().filter(|field| field.name != TAB) {
        properties.insert(field.name.to_string(), field.schema());
        if field.required {
            required.push(field.name);
        }
    }
    Some(json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    }))
}

/// Whether `value` is one to [`MAX_STEPS`] steps, each one a command the
/// contract lists for a batch.
pub(in crate::mcp_broker) fn check_steps(
    tool: &str,
    name: &str,
    value: &Value,
) -> Result<(), String> {
    let steps = value.as_array().ok_or_else(|| {
        format!("{tool}: '{name}' must be a list of {MIN_STEPS} to {MAX_STEPS} steps.")
    })?;
    if !(MIN_STEPS..=MAX_STEPS).contains(&steps.len()) {
        return Err(format!(
            "{tool}: '{name}' must hold {MIN_STEPS} to {MAX_STEPS} steps, not {}.",
            steps.len()
        ));
    }
    for (index, step) in steps.iter().enumerate() {
        check_step(tool, index, step)?;
    }
    Ok(())
}

fn check_step(tool: &str, index: usize, step: &Value) -> Result<(), String> {
    let step = step
        .as_object()
        .ok_or_else(|| format!("{tool}: step {index} must be an object naming one command."))?;
    let command = match step.get("command") {
        Some(Value::String(command)) => command.as_str(),
        Some(_) => return Err(format!("{tool}: step {index} must name 'command' as text.")),
        None => return Err(format!("{tool}: step {index} must name a 'command'.")),
    };
    let Some(spec) = step_spec(command) else {
        return Err(not_a_step_command(tool, index, command));
    };
    if step.contains_key(TAB) {
        return Err(format!(
            "{tool}: step {index} must not name a browserId: the batch's browserId is the tab every step runs on."
        ));
    }
    let mut args = step.clone();
    args.remove("command");
    args.insert(TAB.to_string(), json!(BATCH_TAB));
    parse(spec, tool, &Value::Object(args))
        .map(|_| ())
        .map_err(|sentence| format!("{tool}: step {index}: {sentence}"))
}

/// The row a step's own command declares, and `None` for a command a batch may
/// not run — a name that is not a command of the lane's, or one of the ten that
/// has no row, which the served table keeps impossible.
fn step_spec(command: &str) -> Option<&'static Spec> {
    if ACT_COMMANDS.contains(&command) {
        spec_of(command)
    } else {
        None
    }
}

fn not_a_step_command(tool: &str, index: usize, command: &str) -> String {
    format!(
        "{tool}: step {index} names '{command}', which is not one of {}.",
        ACT_COMMANDS.join(", ")
    )
}
