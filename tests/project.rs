use greentic_dw_authoring::{project, AgentKind, DeepWorkerSpec, LlmRef, WorkerSpec};

fn base(kind: AgentKind) -> WorkerSpec {
    WorkerSpec {
        kind,
        name: "w".into(),
        description: None,
        tenant: None,
        llm: LlmRef {
            provider: "openai".into(),
            model: "gpt-4o".into(),
            credential_ref: None,
        },
        instructions: "do things".into(),
        prompt_mode: Default::default(),
        tone: None,
        tools: vec![],
        memory: None,
        knowledge: None,
        guardrails: vec![],
        agent_graph: None,
        deep_worker: None,
        locale: None,
        icon: None,
        vertical: None,
        opening_message: None,
        extension_tools: vec![],
    }
}

#[test]
fn single_turn_has_no_executing_node() {
    assert!(project::executing_node(&base(AgentKind::SingleTurn)).is_none());
}

#[test]
fn agent_graph_executing_node() {
    let n = project::executing_node(&base(AgentKind::AgentGraph)).unwrap();
    assert_eq!(n["kind"], "dw.agent_graph");
}

#[test]
fn deep_worker_executing_node_carries_config() {
    let mut s = base(AgentKind::DeepWorker);
    s.deep_worker = Some(DeepWorkerSpec {
        iteration_budget: 12,
        ..Default::default()
    });
    let n = project::executing_node(&s).unwrap();
    assert_eq!(n["kind"], "operala.call");
    assert_eq!(n["deep_worker"]["iterationBudget"], 12);
}

#[test]
fn answer_document_has_manifest_id_and_display_name() {
    let doc = project::to_answer_document(&base(AgentKind::SingleTurn)).unwrap();
    assert!(doc["manifest_id"].is_string());
    assert_eq!(doc["display_name"], "w");
}

/// `to_answer_document`'s guardrail projection forwards `cap_id` + `config`
/// for both guardrail shapes: a bare capability-id string maps to
/// `config: null` (matching the Designer's `GuardrailFormRef` default), and
/// the full `{cap_id, config}` shape forwards its config verbatim.
#[test]
fn answer_document_guardrails_carry_cap_id_and_config() {
    use greentic_dw_authoring::GuardrailRefSpec;

    let mut spec = base(AgentKind::SingleTurn);
    spec.guardrails = vec![
        "pii-redact".into(),
        GuardrailRefSpec::Full {
            cap_id: "profanity-filter".into(),
            config: serde_json::json!({ "threshold": 0.8 }),
        },
    ];

    let doc = project::to_answer_document(&spec).unwrap();
    let guardrails = doc["guardrails"].as_array().expect("guardrails array");
    assert_eq!(guardrails.len(), 2);
    assert_eq!(guardrails[0]["cap_id"], "pii-redact");
    assert_eq!(guardrails[0]["config"], serde_json::Value::Null);
    assert_eq!(guardrails[1]["cap_id"], "profanity-filter");
    assert_eq!(guardrails[1]["config"]["threshold"], 0.8);
}
