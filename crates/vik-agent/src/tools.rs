use std::fmt;
use std::time::Duration;

use serde_json::{Value, json};
use vik_workflow::TrackerConfig;

const LINEAR_GRAPHQL_TOOL: &str = "linear_graphql";

#[derive(Clone, Default)]
pub(crate) struct DynamicTools {
    linear_graphql: Option<LinearGraphqlTool>,
}

impl fmt::Debug for DynamicTools {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DynamicTools")
            .field("linear_graphql", &self.linear_graphql.is_some())
            .finish()
    }
}

impl DynamicTools {
    pub(crate) fn from_tracker_config(config: &TrackerConfig) -> Self {
        let linear_graphql = if config.kind == "linear"
            && !config.endpoint.trim().is_empty()
            && !config.api_key.trim().is_empty()
        {
            match LinearGraphqlTool::new(config.endpoint.clone(), config.api_key.clone()) {
                Ok(tool) => Some(tool),
                Err(err) => {
                    tracing::warn!(error = %err, "linear_graphql tool disabled");
                    None
                }
            }
        } else {
            None
        };
        Self { linear_graphql }
    }

    pub(crate) fn definitions(&self) -> Vec<Value> {
        let mut definitions = Vec::new();
        if self.linear_graphql.is_some() {
            definitions.push(json!({
                "name": LINEAR_GRAPHQL_TOOL,
                "description": "Run one Linear GraphQL query or mutation using Vik's configured Linear credentials.",
                "deferLoading": false,
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
            }));
        }
        definitions
    }

    pub(crate) async fn handle_call(&self, params: &Value) -> Value {
        let Some(tool) = extract_tool_name(params) else {
            return tool_failure("missing dynamic tool name");
        };
        if tool != LINEAR_GRAPHQL_TOOL {
            return tool_failure(format!("unsupported dynamic tool call: {tool}"));
        }
        let Some(linear_graphql) = &self.linear_graphql else {
            return tool_failure("linear_graphql tool is not configured");
        };
        let arguments = match extract_tool_arguments(params) {
            Ok(arguments) => arguments,
            Err(err) => return tool_failure(err),
        };
        linear_graphql.call(arguments).await
    }
}

#[derive(Clone)]
struct LinearGraphqlTool {
    http: reqwest::Client,
    endpoint: String,
    api_key: String,
}

impl fmt::Debug for LinearGraphqlTool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LinearGraphqlTool")
            .field("endpoint", &self.endpoint)
            .field("api_key", &"<redacted>")
            .finish()
    }
}

impl LinearGraphqlTool {
    fn new(endpoint: String, api_key: String) -> Result<Self, reqwest::Error> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_millis(30_000))
            .build()?;
        Ok(Self {
            http,
            endpoint,
            api_key,
        })
    }

    async fn call(&self, arguments: Value) -> Value {
        let Some(query) = arguments
            .get("query")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|query| !query.is_empty())
        else {
            return tool_failure("linear_graphql.query is required");
        };
        let variables = arguments
            .get("variables")
            .cloned()
            .unwrap_or_else(|| json!({}));
        if !variables.is_object() {
            return tool_failure("linear_graphql.variables must be an object");
        }
        let body = json!({ "query": query, "variables": variables });
        let response = match self
            .http
            .post(&self.endpoint)
            .header("Authorization", &self.api_key)
            .json(&body)
            .send()
            .await
        {
            Ok(response) => response,
            Err(err) => return tool_failure(format!("linear_graphql request failed: {err}")),
        };
        let status = response.status();
        let text = match response.text().await {
            Ok(text) => text,
            Err(err) => {
                return tool_failure(format!("linear_graphql response read failed: {err}"));
            }
        };
        let payload: Value = match serde_json::from_str(&text) {
            Ok(payload) => payload,
            Err(err) => {
                return tool_failure(format!(
                    "linear_graphql response was not JSON: {err}; status: {}",
                    status.as_u16()
                ));
            }
        };
        if let Some(errors) = payload.get("errors") {
            return tool_failure(format!("linear_graphql errors: {}", compact_json(errors)));
        }
        if !status.is_success() {
            return tool_failure(format!("linear_graphql HTTP status: {}", status.as_u16()));
        }
        tool_success(compact_json(&payload))
    }
}

fn extract_tool_name(params: &Value) -> Option<String> {
    [
        "/tool",
        "/name",
        "/toolName",
        "/tool_name",
        "/tool/name",
        "/toolCall/name",
        "/function/name",
    ]
    .into_iter()
    .find_map(|path| {
        params
            .pointer(path)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(ToOwned::to_owned)
    })
}

fn extract_tool_arguments(params: &Value) -> Result<Value, String> {
    let Some(arguments) = [
        "/arguments",
        "/input",
        "/args",
        "/toolInput",
        "/tool/arguments",
        "/toolCall/arguments",
        "/function/arguments",
    ]
    .into_iter()
    .find_map(|path| params.pointer(path)) else {
        return Ok(json!({}));
    };
    match arguments {
        Value::Object(_) => Ok(arguments.clone()),
        Value::String(raw) => serde_json::from_str(raw)
            .map_err(|err| format!("dynamic tool arguments are not valid JSON: {err}")),
        Value::Null => Ok(json!({})),
        _ => Err("dynamic tool arguments must be an object".to_string()),
    }
}

fn tool_success(text: String) -> Value {
    json!({
        "success": true,
        "contentItems": [
            { "type": "inputText", "text": text }
        ]
    })
}

fn tool_failure(message: impl Into<String>) -> Value {
    json!({
        "success": false,
        "contentItems": [
            { "type": "inputText", "text": message.into() }
        ]
    })
}

fn compact_json(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn linear_tracker_config() -> TrackerConfig {
        TrackerConfig {
            kind: "linear".to_string(),
            endpoint: "https://api.linear.app/graphql".to_string(),
            api_key: "lin_api_key".to_string(),
            project_slug: "VIK".to_string(),
            repository: String::new(),
            active_states: vec!["Todo".to_string()],
            terminal_states: vec!["Done".to_string()],
            filter: Default::default(),
        }
    }

    #[test]
    fn linear_graphql_definition_is_exposed_when_configured() {
        let tools = DynamicTools::from_tracker_config(&linear_tracker_config());
        let definitions = tools.definitions();
        assert_eq!(definitions[0]["name"], LINEAR_GRAPHQL_TOOL);
        assert_eq!(
            definitions[0].pointer("/inputSchema/required/0"),
            Some(&json!("query"))
        );
    }

    #[test]
    fn linear_graphql_definition_is_hidden_for_github_tracker() {
        let mut config = linear_tracker_config();
        config.kind = "github".to_string();
        config.repository = "yii-labs/vik".to_string();
        let tools = DynamicTools::from_tracker_config(&config);

        assert!(tools.definitions().is_empty());
    }

    #[test]
    fn app_server_tool_call_shape_is_extracted() {
        let params = json!({
            "tool": LINEAR_GRAPHQL_TOOL,
            "arguments": { "query": "query { viewer { id } }" }
        });
        assert_eq!(
            extract_tool_name(&params).as_deref(),
            Some(LINEAR_GRAPHQL_TOOL)
        );
        assert_eq!(
            extract_tool_arguments(&params).unwrap()["query"],
            "query { viewer { id } }"
        );
    }

    #[test]
    fn string_arguments_are_parsed() {
        let params = json!({
            "tool": LINEAR_GRAPHQL_TOOL,
            "arguments": "{\"query\":\"query { viewer { id } }\"}"
        });
        assert_eq!(
            extract_tool_arguments(&params).unwrap()["query"],
            "query { viewer { id } }"
        );
    }

    #[tokio::test]
    async fn missing_query_returns_tool_failure_without_network() {
        let tools = DynamicTools::from_tracker_config(&linear_tracker_config());
        let result = tools
            .handle_call(&json!({
                "tool": LINEAR_GRAPHQL_TOOL,
                "arguments": {}
            }))
            .await;
        assert_eq!(result["success"], false);
        assert_eq!(
            result.pointer("/contentItems/0/text"),
            Some(&json!("linear_graphql.query is required"))
        );
    }
}
