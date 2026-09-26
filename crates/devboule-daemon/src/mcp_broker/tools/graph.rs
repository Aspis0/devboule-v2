//! Project-graph and Oracle search tools: their arguments and their reply shape.

use serde_json::{json, Value};

use crate::mcp_broker::dispatch::{rpc_error, tool_error};

/// The arguments of a project-graph call, absent when the request carries
/// none: the tools' own parser is the one place that decides what their closed
/// argument set is, exactly as the create tool's parser does.
pub(in crate::mcp_broker) fn project_graph_arguments(message: &Value) -> Value {
    message
        .pointer("/params/arguments")
        .cloned()
        .unwrap_or(Value::Null)
}

/// One reply shape for the three project-graph tools and the Oracle search
/// (`GraphError` is the shared refusal type, kept as named): their own document
/// on success, `-32602` for a malformed request, and a tool error
/// (`isError: true`) carrying the sentence that names the missing fact when the
/// answer cannot be produced. A refusal is deliberately not an empty document:
/// `[]` would read as "this node has no neighbours".
pub(in crate::mcp_broker) fn project_graph_reply(
    id: &Value,
    result: Result<Value, crate::mcp_project_graph::GraphError>,
) -> Result<Option<Value>, Value> {
    match result {
        Ok(document) => {
            let text = serde_json::to_string(&document).map_err(|error| {
                json!({"jsonrpc":"2.0", "id": id, "error": {"code": -32603, "message": format!("Could not encode project graph: {error}")}})
            })?;
            Ok(Some(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "content": [{"type": "text", "text": text}],
                    "structuredContent": document,
                    "isError": false,
                },
            })))
        }
        Err(crate::mcp_project_graph::GraphError::Invalid(message)) => {
            Ok(Some(rpc_error(id.clone(), -32602, &message)))
        }
        Err(crate::mcp_project_graph::GraphError::Refused(message)) => {
            Ok(Some(tool_error(id, &message)))
        }
    }
}
