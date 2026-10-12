use greentic_dw_authoring::{
    AgentKind, ExtensionToolBinding, GuardrailRefSpec, MemorySpec, ShortTermSpec, WorkerSpec,
};
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

/// Backward-compat: `guardrails: [pii-redact]` (bare capability-id strings,
/// the pre-existing YAML shape another repo already depends on) still
/// parses, alongside the new `{cap_id, config}` object shape in the same
/// list.
#[test]
fn guardrails_parse_bare_string_and_full_object_forms() {
    let yaml = r#"
apiVersion: greentic.ai/v1
kind: single_turn
name: triage
llm: { provider: openai, model: gpt-4o }
instructions: "You are a triage agent."
guardrails:
  - pii-redact
  - cap_id: profanity-filter
    config: { threshold: 0.8 }
"#;
    let spec: WorkerSpec = serde_yaml_bw::from_str(yaml).expect("parse");
    assert_eq!(spec.guardrails.len(), 2);

    assert_eq!(spec.guardrails[0].cap_id(), "pii-redact");
    assert_eq!(spec.guardrails[0].config(), serde_json::Value::Null);
    assert!(matches!(spec.guardrails[0], GuardrailRefSpec::CapId(_)));

    assert_eq!(spec.guardrails[1].cap_id(), "profanity-filter");
    assert_eq!(spec.guardrails[1].config()["threshold"], 0.8);
    assert!(matches!(spec.guardrails[1], GuardrailRefSpec::Full { .. }));
}

/// Backward-compat: `short_term: true` / `short_term: false` (the
/// pre-existing bare-bool shape) still parse, alongside the new named
/// `ProviderRef` object shape.
#[test]
fn short_term_parses_bare_bool_and_named_provider_forms() {
    let enabled: MemorySpec = serde_yaml_bw::from_str("short_term: true\n").expect("parse");
    assert_eq!(enabled.short_term, ShortTermSpec::Enabled(true));

    let disabled: MemorySpec = serde_yaml_bw::from_str("short_term: false\n").expect("parse");
    assert_eq!(disabled.short_term, ShortTermSpec::Enabled(false));

    let named: MemorySpec = serde_yaml_bw::from_str(
        "short_term:\n  provider: redis\n  credential_ref: vault://acme/redis\n  params: { ttl_seconds: 60 }\n",
    )
    .expect("parse");
    match named.short_term {
        ShortTermSpec::Provider(provider) => {
            assert_eq!(provider.provider, "redis");
            assert_eq!(
                provider.credential_ref.as_deref(),
                Some("vault://acme/redis")
            );
            assert_eq!(provider.params.get("ttl_seconds").unwrap(), 60);
        }
        other => panic!("expected ShortTermSpec::Provider, got {other:?}"),
    }
}

/// Absent `short_term` defaults to disabled, matching the pre-existing
/// bare-`bool` default of `false`.
#[test]
fn short_term_defaults_to_disabled_when_absent() {
    let mem: MemorySpec = serde_yaml_bw::from_str("{}\n").expect("parse");
    assert_eq!(mem.short_term, ShortTermSpec::Enabled(false));
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
        prompt_mode: Default::default(),
        tone: None,
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
