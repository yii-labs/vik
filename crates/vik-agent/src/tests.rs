use serde_json::json;

use crate::event::extract_usage;

#[test]
fn token_usage_prefers_absolute_totals() {
    let event = json!({
        "method": "thread/tokenUsage/updated",
        "params": {
            "total_token_usage": {
                "input_tokens": 10,
                "output_tokens": 5,
                "total_tokens": 15
            }
        }
    });
    let usage = extract_usage("thread/tokenUsage/updated", &event).unwrap();
    assert_eq!(usage.total_tokens, 15);
}

#[test]
fn session_id_composes_thread_and_turn() {
    assert_eq!(
        vik_core::session_id("thread-1", "turn-2"),
        "thread-1-turn-2"
    );
}

#[test]
fn thread_start_payload_uses_workspace_cwd() {
    let payload = json!({
        "cwd": "/tmp/workspace",
        "ephemeral": true
    });
    assert_eq!(payload["cwd"], "/tmp/workspace");
}
