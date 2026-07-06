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
