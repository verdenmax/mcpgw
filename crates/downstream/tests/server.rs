mod common;

use std::sync::Arc;

use gateway::GatewayState;

#[tokio::test]
async fn list_tools_returns_exactly_the_three_metatools() {
    let state = Arc::new(GatewayState::new("bm25").unwrap());
    let client = common::connect_to_gateway(state, 8).await;

    let tools = client.list_all_tools().await.unwrap();
    let mut names: Vec<String> = tools.iter().map(|t| t.name.to_string()).collect();
    names.sort();
    assert_eq!(names, ["call_tool", "get_tool_details", "search_tools"]);

    client.cancel().await.unwrap();
}

use rmcp::model::CallToolRequestParams;
use serde_json::json;

fn args(v: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
    v.as_object().unwrap().clone()
}

#[tokio::test]
async fn call_tool_dispatches_all_three_metatools() {
    let state = Arc::new(GatewayState::new("bm25").unwrap());
    common::attach_mock(&state, "mock").await;
    let client = common::connect_to_gateway(state, 8).await;

    // search_tools finds echo.
    let r = client
        .call_tool(
            CallToolRequestParams::new("search_tools")
                .with_arguments(args(json!({"query":"echo"}))),
        )
        .await
        .unwrap();
    assert_ne!(r.is_error, Some(true));
    let text = r.content[0].as_text().unwrap().text.clone();
    assert!(text.contains("mock__echo"), "search result: {text}");

    // get_tool_details returns echo's def.
    let r = client
        .call_tool(
            CallToolRequestParams::new("get_tool_details")
                .with_arguments(args(json!({"name":"mock__echo"}))),
        )
        .await
        .unwrap();
    assert_ne!(r.is_error, Some(true));
    assert!(r.content[0].as_text().unwrap().text.contains("echo"));

    // call_tool forwards to upstream echo.
    let r = client
        .call_tool(
            CallToolRequestParams::new("call_tool").with_arguments(args(json!({
                "name": "mock__echo", "arguments": {"text": "hi"}
            }))),
        )
        .await
        .unwrap();
    assert_ne!(r.is_error, Some(true));
    assert!(r.content[0].as_text().unwrap().text.contains("hi"));

    client.cancel().await.unwrap();
}

#[tokio::test]
async fn call_tool_unknown_meta_name_is_protocol_error() {
    let state = Arc::new(GatewayState::new("bm25").unwrap());
    let client = common::connect_to_gateway(state, 8).await;
    let err = client
        .call_tool(CallToolRequestParams::new("does_not_exist"))
        .await;
    assert!(err.is_err(), "unknown meta-tool must be a protocol error");
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn call_tool_routes_missing_upstream_tool_to_iserror() {
    let state = Arc::new(GatewayState::new("bm25").unwrap());
    common::attach_mock(&state, "mock").await;
    let client = common::connect_to_gateway(state, 8).await;
    let r = client
        .call_tool(
            CallToolRequestParams::new("call_tool")
                .with_arguments(args(json!({"name":"mock__nope"}))),
        )
        .await
        .unwrap();
    assert_eq!(r.is_error, Some(true)); // MetaError::ToolNotFound -> isError
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn list_changed_refreshes_what_search_can_find() {
    use std::time::Duration;
    let state = Arc::new(GatewayState::new("bm25").unwrap());
    common::attach_revealing_mock_with_worker(&state, "mock").await;
    let client = common::connect_to_gateway(state.clone(), 8).await;

    // Initially: late_tool is not revealed, so search can't find it.
    let r = client
        .call_tool(
            CallToolRequestParams::new("search_tools")
                .with_arguments(args(json!({"query":"late_tool"}))),
        )
        .await
        .unwrap();
    assert!(!r.content[0]
        .as_text()
        .unwrap()
        .text
        .contains("mock__late_tool"));

    // Call the upstream's reveal THROUGH the gateway -> upstream emits tools/list_changed
    // -> handler -> trigger -> worker rebuilds.
    client
        .call_tool(
            CallToolRequestParams::new("call_tool")
                .with_arguments(args(json!({"name":"mock__reveal"}))),
        )
        .await
        .unwrap();

    // Poll until search surfaces the newly revealed tool.
    let mut found = false;
    for _ in 0..100 {
        let r = client
            .call_tool(
                CallToolRequestParams::new("search_tools")
                    .with_arguments(args(json!({"query":"late_tool revealed runtime"}))),
            )
            .await
            .unwrap();
        if r.content[0]
            .as_text()
            .unwrap()
            .text
            .contains("mock__late_tool")
        {
            found = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        found,
        "after list_changed, search_tools should surface mock__late_tool"
    );

    client.cancel().await.unwrap();
}

#[tokio::test]
async fn runtime_upstream_crash_is_isolated_from_other_upstreams() {
    // Two live upstreams; one crashes mid-session. The dead one's tool must self-heal to an
    // isError (no panic / no hang), and the OTHER upstream must keep working (isolation).
    let state = Arc::new(GatewayState::new("bm25").unwrap());
    common::attach_mock(&state, "alive").await; // rebuilds with alive's tools
    let doomed = common::attach_killable_mock(&state, "doomed").await; // no rebuild
    state.rebuild_snapshot().await.unwrap(); // catalog now has alive__* + doomed__*

    let client = common::connect_to_gateway(state, 8).await;

    // Sanity: the doomed upstream is callable BEFORE the crash.
    let r = client
        .call_tool(
            CallToolRequestParams::new("call_tool").with_arguments(args(json!({
                "name": "doomed__echo", "arguments": {"text": "pre"}
            }))),
        )
        .await
        .unwrap();
    assert_ne!(r.is_error, Some(true), "doomed__echo should work pre-crash");

    // Crash it: aborting the server task drops the duplex, closing the connection.
    doomed.abort();
    let _ = doomed.await; // ensure the abort has settled before we call again

    // The catalog entry still exists (no rebuild happened), so the isError below is attributable
    // to the dead connection, not a missing tool.
    let details = client
        .call_tool(
            CallToolRequestParams::new("get_tool_details")
                .with_arguments(args(json!({"name": "doomed__echo"}))),
        )
        .await
        .unwrap();
    assert_ne!(
        details.is_error,
        Some(true),
        "doomed__echo must still be in the catalog post-crash"
    );

    // The dead upstream's tool now gracefully degrades to an isError result (not a panic / hang).
    let r = client
        .call_tool(
            CallToolRequestParams::new("call_tool").with_arguments(args(json!({
                "name": "doomed__echo", "arguments": {"text": "post"}
            }))),
        )
        .await
        .unwrap();
    assert_eq!(
        r.is_error,
        Some(true),
        "a call to a crashed upstream must gracefully degrade to isError"
    );

    // Isolation: the other upstream is unaffected and still forwards normally.
    let r = client
        .call_tool(
            CallToolRequestParams::new("call_tool").with_arguments(args(json!({
                "name": "alive__echo", "arguments": {"text": "still-here"}
            }))),
        )
        .await
        .unwrap();
    assert_ne!(
        r.is_error,
        Some(true),
        "a live upstream must be unaffected by another's crash"
    );
    assert!(r.content[0].as_text().unwrap().text.contains("still-here"));

    client.cancel().await.unwrap();
}

#[tokio::test]
async fn meta_tool_calls_are_observed_with_metadata() {
    use observe::{CallOutcome, MetaTool};
    let state = Arc::new(GatewayState::new("bm25").unwrap());
    common::attach_mock(&state, "mock").await;

    let cap = observe::CaptureSink::new();
    let sinks: Arc<[Arc<dyn observe::CallSink>]> =
        vec![Arc::new(cap.clone()) as Arc<dyn observe::CallSink>].into();
    let client = common::connect_to_gateway_with_sinks(state, 8, sinks).await;

    let _ = client
        .call_tool(
            CallToolRequestParams::new("search_tools")
                .with_arguments(args(json!({"query": "echo"}))),
        )
        .await
        .unwrap();
    let _ = client
        .call_tool(
            CallToolRequestParams::new("call_tool").with_arguments(args(json!({
                "name": "mock__echo", "arguments": {"text": "hi"}
            }))),
        )
        .await
        .unwrap();
    let _ = client
        .call_tool(
            CallToolRequestParams::new("call_tool")
                .with_arguments(args(json!({"name": "mock__nope"}))),
        )
        .await
        .unwrap();
    // An unknown meta-tool name is a protocol error, not a gateway call, and is NOT recorded.
    let unknown = client
        .call_tool(CallToolRequestParams::new("not_a_meta_tool"))
        .await;
    assert!(
        unknown.is_err(),
        "unknown meta-tool should be a protocol error"
    );
    client.cancel().await.unwrap();

    let recs = cap.records();
    assert_eq!(
        recs.len(),
        3,
        "one record per gateway meta-tool call; the unknown name is not recorded"
    );

    let search = &recs[0];
    assert_eq!(search.meta_tool, MetaTool::SearchTools);
    assert_eq!(search.outcome, CallOutcome::Ok);
    assert!(search.target_tool.is_none() && search.upstream.is_none());
    assert!(search.arg_bytes > 0);

    let call_ok = &recs[1];
    assert_eq!(call_ok.meta_tool, MetaTool::CallTool);
    assert_eq!(call_ok.outcome, CallOutcome::Ok);
    assert_eq!(call_ok.target_tool.as_deref(), Some("mock__echo"));
    assert_eq!(call_ok.upstream.as_deref(), Some("mock"));
    assert!(call_ok.error_kind.is_none());
    assert!(call_ok.result_bytes > 0);

    let call_err = &recs[2];
    assert_eq!(call_err.meta_tool, MetaTool::CallTool);
    assert_eq!(call_err.outcome, CallOutcome::Error);
    assert_eq!(call_err.error_kind, Some("tool_not_found"));
    assert_eq!(call_err.upstream.as_deref(), Some("mock"));
}

#[tokio::test]
async fn upstream_tool_error_is_recorded_as_error_outcome() {
    use observe::{CallOutcome, MetaTool};
    let state = Arc::new(GatewayState::new("bm25").unwrap());
    common::attach_mock(&state, "mock").await;

    let cap = observe::CaptureSink::new();
    let sinks: Arc<[Arc<dyn observe::CallSink>]> =
        vec![Arc::new(cap.clone()) as Arc<dyn observe::CallSink>].into();
    let client = common::connect_to_gateway_with_sinks(state, 8, sinks).await;

    let r = client
        .call_tool(
            CallToolRequestParams::new("call_tool")
                .with_arguments(args(json!({"name": "mock__fail"}))),
        )
        .await
        .unwrap();
    // The tool-level error is forwarded to the client unchanged.
    assert_eq!(r.is_error, Some(true));
    client.cancel().await.unwrap();

    let recs = cap.records();
    assert_eq!(recs.len(), 1, "exactly one record for the single call_tool");
    let rec = &recs[0];
    assert_eq!(rec.meta_tool, MetaTool::CallTool);
    assert_eq!(rec.target_tool.as_deref(), Some("mock__fail"));
    assert_eq!(rec.upstream.as_deref(), Some("mock"));
    assert_eq!(rec.outcome, CallOutcome::Error);
    assert_eq!(rec.error_kind, Some("upstream_tool_error"));
}

#[tokio::test]
async fn timeout_call_is_recorded_with_timeout_error_kind() {
    use observe::{CallOutcome, MetaTool};
    let state = Arc::new(GatewayState::new("bm25").unwrap());
    common::attach_mock_with_timeout(&state, "mock", std::time::Duration::from_millis(50)).await;

    let cap = observe::CaptureSink::new();
    let sinks: Arc<[Arc<dyn observe::CallSink>]> =
        vec![Arc::new(cap.clone()) as Arc<dyn observe::CallSink>].into();
    let client = common::connect_to_gateway_with_sinks(state, 8, sinks).await;

    // `slow` sleeps ~10s server-side; the 50ms call timeout trips MetaError::Timeout.
    let _ = client
        .call_tool(
            CallToolRequestParams::new("call_tool")
                .with_arguments(args(json!({"name": "mock__slow"}))),
        )
        .await
        .unwrap();
    client.cancel().await.unwrap();

    let recs = cap.records();
    let rec = recs.last().expect("a record for the call");
    assert_eq!(rec.meta_tool, MetaTool::CallTool);
    assert_eq!(rec.outcome, CallOutcome::Timeout);
    assert_eq!(rec.error_kind, Some("timeout"));
}

#[tokio::test]
async fn missing_name_is_recorded_with_invalid_params_error_kind() {
    use observe::{CallOutcome, MetaTool};
    let state = Arc::new(GatewayState::new("bm25").unwrap());
    common::attach_mock(&state, "mock").await;

    let cap = observe::CaptureSink::new();
    let sinks: Arc<[Arc<dyn observe::CallSink>]> =
        vec![Arc::new(cap.clone()) as Arc<dyn observe::CallSink>].into();
    let client = common::connect_to_gateway_with_sinks(state, 8, sinks).await;

    // call_tool with no "name" -> the invalid_params arm.
    let _ = client
        .call_tool(CallToolRequestParams::new("call_tool").with_arguments(args(json!({}))))
        .await
        .unwrap();
    client.cancel().await.unwrap();

    let recs = cap.records();
    let rec = recs.last().expect("a record for the call");
    assert_eq!(rec.meta_tool, MetaTool::CallTool);
    assert_eq!(rec.outcome, CallOutcome::Error);
    assert_eq!(rec.error_kind, Some("invalid_params"));
}
