use greentic_dw_authoring::{AgentKind, ExtensionToolBinding, WorkerSpec};
use greentic_extension_sdk_contract::AgenticWorkerMetadata;
use greentic_types::secrets::{SecretKey, SecretRequirement};

#[test]
fn worker_spec_round_trips_yaml() {
    let yaml = r#"
apiVersion: greentic.ai/v1
kind: single_turn
name: triage
llm: { provider: openai, model: gpt-4o, credential_ref: llm-openai }
instructions: "You are a triage agent."
tools: [ web.search ]
"#;
    let spec: WorkerSpec = serde_yaml_bw::from_str(yaml).expect("parse");
    assert_eq!(spec.kind, AgentKind::SingleTurn);
    assert_eq!(spec.name, "triage");
    assert_eq!(spec.llm.provider, "openai");
    assert_eq!(spec.tools, vec!["web.search".to_string()]);
}

#[test]
fn deep_worker_defaults() {
    let spec: WorkerSpec = serde_yaml_bw::from_str(
        "apiVersion: greentic.ai/v1\nkind: deep_worker\nname: r\nllm: {provider: openai, model: gpt-4o}\ninstructions: x\ndeep_worker: {}\n",
    )
    .unwrap();
    let dw = spec.deep_worker.unwrap();
    assert_eq!(dw.iteration_budget, 8);
}

#[test]
fn worker_spec_with_extension_tool_round_trips_json() {
    let binding = ExtensionToolBinding {
        extension_id: "greentic.web".to_string(),
        extension_version: "1.0.0".to_string(),
        tool_name: "search".to_string(),
        description: "Search the web".to_string(),
        input_schema_json: "{\"type\":\"object\"}".to_string(),
        output_schema_json: Some("{\"type\":\"string\"}".to_string()),
        capabilities: vec!["agentic_worker".to_string()],
        agentic_worker_metadata: AgenticWorkerMetadata {
            usage_hint: Some("use for fresh info".to_string()),
            ..Default::default()
        },
        secret_requirements: vec![{
            let mut req = SecretRequirement::default();
            req.key = SecretKey::new("firecrawl-api-key").expect("valid key");
            req.required = true;
            req
        }],
        usage_note: Some("rate limited".to_string()),
    };

    let spec = WorkerSpec {
        kind: AgentKind::SingleTurn,
        name: "researcher".to_string(),
        description: None,
        tenant: None,
        llm: greentic_dw_authoring::LlmRef {
            provider: "openai".to_string(),
            model: "gpt-4o".to_string(),
            credential_ref: None,
        },
        instructions: "Research things.".to_string(),
        tools: vec![],
        memory: None,
        knowledge: None,
        guardrails: vec![],
        agent_graph: None,
        deep_worker: None,
        locale: Some("en-US".to_string()),
        icon: Some("robot".to_string()),
        vertical: Some("research".to_string()),
        opening_message: Some("Hi, how can I help?".to_string()),
        extension_tools: vec![binding],
    };

    let json = serde_json::to_string(&spec).expect("serialize");
    let round_tripped: WorkerSpec = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(spec, round_tripped);
}
