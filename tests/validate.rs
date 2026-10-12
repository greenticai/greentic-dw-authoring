use greentic_dw_authoring::{
    validate, AgentGraphSpec, AgentKind, Coordinator, DeepWorkerSpec, LlmRef, Specialist,
    WorkerSpec,
};

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

fn specialist(name: &str) -> Specialist {
    Specialist {
        name: name.into(),
        instructions: "help".into(),
        tools: vec![],
    }
}

fn agent_graph(specialists: Vec<Specialist>) -> AgentGraphSpec {
    AgentGraphSpec {
        coordinator: Coordinator {
            instructions: "coordinate".into(),
        },
        specialists,
    }
}

// Rule 1: name

#[test]
fn valid_single_turn_spec_passes() {
    assert_eq!(validate(&base(AgentKind::SingleTurn)), Ok(()));
}

#[test]
fn empty_name_is_rejected() {
    let mut s = base(AgentKind::SingleTurn);
    s.name = "".into();
    let errs = validate(&s).unwrap_err();
    assert!(errs.iter().any(|e| e.field == "name"));
}

#[test]
fn whitespace_only_name_is_rejected() {
    let mut s = base(AgentKind::SingleTurn);
    s.name = "   ".into();
    let errs = validate(&s).unwrap_err();
    assert!(errs.iter().any(|e| e.field == "name"));
}

// Rule 2: agent_graph requires >= 2 specialists

#[test]
fn valid_agent_graph_spec_passes() {
    let mut s = base(AgentKind::AgentGraph);
    s.agent_graph = Some(agent_graph(vec![specialist("a"), specialist("b")]));
    assert_eq!(validate(&s), Ok(()));
}

#[test]
fn agent_graph_missing_block_is_rejected() {
    let s = base(AgentKind::AgentGraph);
    let errs = validate(&s).unwrap_err();
    assert!(errs.iter().any(|e| e.field == "agent_graph.specialists"));
}

#[test]
fn agent_graph_with_one_specialist_is_rejected() {
    let mut s = base(AgentKind::AgentGraph);
    s.agent_graph = Some(agent_graph(vec![specialist("a")]));
    let errs = validate(&s).unwrap_err();
    assert!(errs.iter().any(|e| e.field == "agent_graph.specialists"));
}

// Rule 3: duplicate specialist names

#[test]
fn agent_graph_with_duplicate_specialist_names_is_rejected() {
    let mut s = base(AgentKind::AgentGraph);
    s.agent_graph = Some(agent_graph(vec![specialist("dup"), specialist("dup")]));
    let errs = validate(&s).unwrap_err();
    let err = errs
        .iter()
        .find(|e| e.field == "agent_graph.specialists")
        .expect("expected duplicate specialist error");
    assert!(err.message.contains("dup"));
}

// Rule 4: deep_worker iteration_budget range

#[test]
fn valid_deep_worker_spec_passes() {
    let mut s = base(AgentKind::DeepWorker);
    s.deep_worker = Some(DeepWorkerSpec {
        iteration_budget: 8,
        ..Default::default()
    });
    assert_eq!(validate(&s), Ok(()));
}

#[test]
fn deep_worker_iteration_budget_zero_is_rejected() {
    let mut s = base(AgentKind::DeepWorker);
    s.deep_worker = Some(DeepWorkerSpec {
        iteration_budget: 0,
        ..Default::default()
    });
    let errs = validate(&s).unwrap_err();
    assert!(errs
        .iter()
        .any(|e| e.field == "deep_worker.iteration_budget"));
}

#[test]
fn deep_worker_iteration_budget_over_100_is_rejected() {
    let mut s = base(AgentKind::DeepWorker);
    s.deep_worker = Some(DeepWorkerSpec {
        iteration_budget: 101,
        ..Default::default()
    });
    let errs = validate(&s).unwrap_err();
    assert!(errs
        .iter()
        .any(|e| e.field == "deep_worker.iteration_budget"));
}

// Rule 5: kind/block mismatch

#[test]
fn single_turn_with_agent_graph_block_is_rejected() {
    let mut s = base(AgentKind::SingleTurn);
    s.agent_graph = Some(agent_graph(vec![specialist("a"), specialist("b")]));
    let errs = validate(&s).unwrap_err();
    assert!(errs.iter().any(|e| e.field == "kind"));
}

#[test]
fn single_turn_with_deep_worker_block_is_rejected() {
    let mut s = base(AgentKind::SingleTurn);
    s.deep_worker = Some(DeepWorkerSpec::default());
    let errs = validate(&s).unwrap_err();
    assert!(errs.iter().any(|e| e.field == "kind"));
}

#[test]
fn agent_graph_with_deep_worker_block_is_rejected() {
    let mut s = base(AgentKind::AgentGraph);
    s.agent_graph = Some(agent_graph(vec![specialist("a"), specialist("b")]));
    s.deep_worker = Some(DeepWorkerSpec::default());
    let errs = validate(&s).unwrap_err();
    assert!(errs.iter().any(|e| e.field == "kind"));
}

#[test]
fn deep_worker_with_agent_graph_block_is_rejected() {
    let mut s = base(AgentKind::DeepWorker);
    s.deep_worker = Some(DeepWorkerSpec::default());
    s.agent_graph = Some(agent_graph(vec![specialist("a"), specialist("b")]));
    let errs = validate(&s).unwrap_err();
    assert!(errs.iter().any(|e| e.field == "kind"));
}

// Multiple errors collected at once

#[test]
fn all_errors_are_collected_not_just_the_first() {
    let mut s = base(AgentKind::AgentGraph);
    s.name = "".into();
    // agent_graph block missing entirely -> both name and agent_graph.specialists errors expected
    let errs = validate(&s).unwrap_err();
    assert!(errs.iter().any(|e| e.field == "name"));
    assert!(errs.iter().any(|e| e.field == "agent_graph.specialists"));
    assert!(errs.len() >= 2);
}
