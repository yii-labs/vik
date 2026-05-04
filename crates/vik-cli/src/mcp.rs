use std::error::Error;
use std::io::{self, BufRead, Write};

use clap::{Args, Subcommand};
use serde_json::{Value, json};
use vik_tracker::{DEFAULT_LINEAR_ENDPOINT, LinearClient, LinearClientConfig};

const LINEAR_GRAPHQL_TOOL: &str = "linear_graphql";
const LINEAR_GRAPHQL_ENDPOINT_ENV: &str = "VIK_LINEAR_GRAPHQL_ENDPOINT";
const LINEAR_GRAPHQL_API_KEY_ENV: &str = "VIK_LINEAR_GRAPHQL_API_KEY";

#[derive(Debug, Args)]
pub(crate) struct McpArgs {
    #[command(subcommand)]
    command: McpCommand,
}

#[derive(Debug, Subcommand)]
enum McpCommand {
    /// Run the Vik Linear GraphQL MCP server over stdio.
    LinearGraphql,
}

pub(crate) async fn run(args: McpArgs) -> Result<(), Box<dyn Error>> {
    match args.command {
        McpCommand::LinearGraphql => run_linear_graphql().await,
    }
}

async fn run_linear_graphql() -> Result<(), Box<dyn Error>> {
    let endpoint = std::env::var(LINEAR_GRAPHQL_ENDPOINT_ENV)
        .or_else(|_| std::env::var("LINEAR_GRAPHQL_ENDPOINT"))
        .unwrap_or_else(|_| DEFAULT_LINEAR_ENDPOINT.to_string());
    let api_key =
        std::env::var(LINEAR_GRAPHQL_API_KEY_ENV).or_else(|_| std::env::var("LINEAR_API_KEY"))?;
    let client = LinearClient::new(LinearClientConfig::new(
        endpoint,
        api_key,
        "mcp",
        Vec::new(),
    ))?;
    let stdin = io::stdin();
    let mut stdout = io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Value>(&line) {
            Ok(message) => handle_message(&client, message).await,
            Err(err) => Some(error_response(
                json!(null),
                -32700,
                format!("parse error: {err}"),
            )),
        };
        if let Some(response) = response {
            serde_json::to_writer(&mut stdout, &response)?;
            stdout.write_all(b"\n")?;
            stdout.flush()?;
        }
    }
    Ok(())
}

async fn handle_message(client: &LinearClient, message: Value) -> Option<Value> {
    let id = message.get("id").cloned();
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        return id.map(|id| error_response(id, -32600, "missing method"));
    };
    let id = id?;
    match method {
        "initialize" => Some(success_response(
            id,
            json!({
                "protocolVersion": message
                    .pointer("/params/protocolVersion")
                    .and_then(Value::as_str)
                    .unwrap_or("2024-11-05"),
                "capabilities": {
                    "tools": {}
                },
                "serverInfo": {
                    "name": "vik",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }),
        )),
        "ping" => Some(success_response(id, json!({}))),
        "tools/list" => Some(success_response(
            id,
            json!({
                "tools": [linear_graphql_tool_definition()]
            }),
        )),
        "tools/call" => Some(handle_tool_call(client, id, message).await),
        _ => Some(error_response(
            id,
            -32601,
            format!("unsupported MCP method: {method}"),
        )),
    }
}

async fn handle_tool_call(client: &LinearClient, id: Value, message: Value) -> Value {
    let Some(name) = message.pointer("/params/name").and_then(Value::as_str) else {
        return error_response(id, -32602, "tools/call params.name is required");
    };
    if name != LINEAR_GRAPHQL_TOOL {
        return tool_result(id, format!("unsupported tool: {name}"), true);
    }
    let arguments = message
        .pointer("/params/arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let Some(query) = arguments
        .get("query")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|query| !query.is_empty())
    else {
        return tool_result(id, "linear_graphql.query is required", true);
    };
    let variables = arguments
        .get("variables")
        .cloned()
        .unwrap_or_else(|| json!({}));
    if !variables.is_object() {
        return tool_result(id, "linear_graphql.variables must be an object", true);
    }
    match client.graphql(query, variables).await {
        Ok(payload) => tool_result(id, compact_json(&payload), false),
        Err(err) => tool_result(id, err.to_string(), true),
    }
}

fn linear_graphql_tool_definition() -> Value {
    json!({
        "name": LINEAR_GRAPHQL_TOOL,
        "description": "Run one Linear GraphQL query or mutation using Vik's configured Linear credentials.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "GraphQL query or mutation document."
                },
                "variables": {
                    "type": "object",
                    "description": "Optional GraphQL variables object.",
                    "additionalProperties": true
                }
            },
            "required": ["query"],
            "additionalProperties": false
        }
    })
}

fn success_response(id: Value, result: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result
    })
}

fn tool_result(id: Value, text: impl Into<String>, is_error: bool) -> Value {
    success_response(
        id,
        json!({
            "content": [
                {
                    "type": "text",
                    "text": text.into()
                }
            ],
            "isError": is_error
        }),
    )
}

fn error_response(id: Value, code: i64, message: impl Into<String>) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": code,
            "message": message.into()
        }
    })
}

fn compact_json(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_graphql_tool_definition_matches_vik_tool_name() {
        let definition = linear_graphql_tool_definition();
        assert_eq!(definition["name"], "linear_graphql");
        assert_eq!(
            definition.pointer("/inputSchema/required/0"),
            Some(&json!("query"))
        );
    }

    #[test]
    fn tool_result_uses_mcp_content_shape() {
        let result = tool_result(json!(1), "ok", false);
        assert_eq!(
            result.pointer("/result/content/0/type"),
            Some(&json!("text"))
        );
        assert_eq!(result.pointer("/result/content/0/text"), Some(&json!("ok")));
        assert_eq!(result.pointer("/result/isError"), Some(&json!(false)));
    }
}
