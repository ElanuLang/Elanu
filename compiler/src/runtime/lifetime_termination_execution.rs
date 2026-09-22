use super::*;
use crate::check_source_with_runtime_models;

// Runtime-semantic experiment only: these tests define the provisional lifetime
// candidate without making termination reachable from compiler-accepted source.
// Duplicate and cross-owner cleanup below protects the established rule that
// structural membership is non-owning and must not become lifetime authority.
fn runtime(source: &str) -> Runtime {
    let checked = check_source_with_runtime_models(source).expect("source should check");
    Runtime::from_checked_source(&checked).expect("runtime should initialize")
}

fn run_test_transaction(
    runtime: &mut Runtime,
    operation: impl FnOnce(&mut Runtime) -> Result<(), RuntimeError>,
) -> Result<(), RuntimeError> {
    assert!(runtime.transaction.is_none());
    runtime.transaction = Some(Transaction::default());
    match operation(runtime) {
        Ok(()) => {
            let transaction = runtime
                .transaction
                .take()
                .expect("transaction should exist");
            runtime.commit(transaction);
            Ok(())
        }
        Err(error) => {
            runtime.transaction = None;
            Err(error)
        }
    }
}

fn targets(runtime: &mut Runtime, name: &str) -> Vec<String> {
    let Value::Sequence { targets, .. } = runtime.value(name).expect("sequence should exist")
    else {
        panic!("{name} should be a runtime sequence");
    };
    targets
}

const OPTIONAL_SOURCE: &str = r#"
state model Task {
    state title = ""
}

state model Board {
    state tasks: [live Task] = []
    state trash: [live Task] = []
}

state left: Board
state right: Board
state selected: maybe live Task = none
state recent: maybe live Task = none

action seed {
    create Task in left as task {
        through task.title = "Draft"
        insert task into left.tasks
        insert task into right.tasks
        insert task into right.trash
        insert task into right.trash
    }
    selected = left.tasks[0]
    recent = left.tasks[0]
}
"#;

#[test]
fn termination_clears_optional_designations_all_memberships_and_child_state_atomically() {
    let mut runtime = runtime(OPTIONAL_SOURCE);
    runtime.run_action("seed").expect("seed should commit");

    let identity = targets(&mut runtime, "__meld_mseq$left$tasks")[0].clone();
    let title_state = model_binding_name(&identity, "title");

    runtime.transaction = Some(Transaction::default());
    runtime
        .terminate_runtime_model(&identity, "left")
        .expect("leaf child with only optional persistent designations should terminate");

    assert_eq!(
        runtime.value("__meld_live$selected").unwrap(),
        Value::String(String::new())
    );
    assert_eq!(
        runtime.value("__meld_live$recent").unwrap(),
        Value::String(String::new())
    );
    assert!(targets(&mut runtime, "__meld_mseq$left$tasks").is_empty());
    assert!(targets(&mut runtime, "__meld_mseq$right$tasks").is_empty());
    assert!(targets(&mut runtime, "__meld_mseq$right$trash").is_empty());
    assert!(!runtime.model_identity_exists(&identity));
    assert!(runtime.read_name(&title_state, None).is_err());

    let transaction = runtime
        .transaction
        .take()
        .expect("transaction should exist");
    runtime.commit(transaction);

    assert!(!runtime.dynamic_model_owners.contains_key(&identity));
    assert!(!runtime.states.contains_key(&title_state));
    assert!(targets(&mut runtime, "__meld_mseq$left$tasks").is_empty());
    assert!(targets(&mut runtime, "__meld_mseq$right$tasks").is_empty());
    assert!(targets(&mut runtime, "__meld_mseq$right$trash").is_empty());
}

#[test]
fn aborted_termination_restores_child_memberships_and_optional_designations() {
    let mut runtime = runtime(OPTIONAL_SOURCE);
    runtime.run_action("seed").expect("seed should commit");

    let identity = targets(&mut runtime, "__meld_mseq$left$tasks")[0].clone();
    let title_state = model_binding_name(&identity, "title");

    runtime.transaction = Some(Transaction::default());
    runtime
        .terminate_runtime_model(&identity, "left")
        .expect("termination should stage successfully");
    assert!(runtime.read_name(&title_state, None).is_err());
    runtime.transaction = None;

    assert!(runtime.model_identity_exists(&identity));
    assert_eq!(
        runtime.value("__meld_live$selected").unwrap(),
        Value::String(identity.clone())
    );
    assert_eq!(
        runtime.value("__meld_live$recent").unwrap(),
        Value::String(identity.clone())
    );
    assert_eq!(
        targets(&mut runtime, "__meld_mseq$left$tasks"),
        vec![identity.clone()]
    );
    assert_eq!(
        targets(&mut runtime, "__meld_mseq$right$tasks"),
        vec![identity.clone()]
    );
    assert_eq!(
        targets(&mut runtime, "__meld_mseq$right$trash"),
        vec![identity.clone(), identity.clone()]
    );
    assert_eq!(
        runtime.read_name(&title_state, None).unwrap(),
        Value::String("Draft".into())
    );
}

#[test]
fn plain_live_designation_blocks_termination_and_prior_writes_roll_back() {
    let mut runtime = runtime(
        r#"
state model Task {
    state title = ""
}

state model Board {
    state tasks: [live Task] = []
}

state fallback: Task
state board: Board
state required: live Task = live fallback
state selected: maybe live Task = none
state marker = 0

action seed {
    create Task in board as task {
        through task.title = "Kept"
        insert task into board.tasks
    }
    required = board.tasks[0]
    selected = board.tasks[0]
}
"#,
    );
    runtime.run_action("seed").expect("seed should commit");
    let identity = targets(&mut runtime, "__meld_mseq$board$tasks")[0].clone();

    let error = run_test_transaction(&mut runtime, |runtime| {
        runtime.write_state("marker", Value::Int(1))?;
        runtime.terminate_runtime_model(&identity, "board")
    })
    .expect_err("plain live designation must block lifetime termination");

    assert!(error.message.contains("plain live designation"));
    assert_eq!(runtime.value("marker").unwrap(), Value::Int(0));
    assert!(runtime.model_identity_exists(&identity));
    assert_eq!(
        runtime.value("__meld_live$required").unwrap(),
        Value::String(identity.clone())
    );
    assert_eq!(
        targets(&mut runtime, "__meld_mseq$board$tasks"),
        vec![identity]
    );
}

#[test]
fn rooted_descendant_blocks_non_cascading_termination() {
    let mut runtime = runtime(OPTIONAL_SOURCE);
    runtime.run_action("seed").expect("seed should commit");
    let identity = targets(&mut runtime, "__meld_mseq$left$tasks")[0].clone();

    runtime.transaction = Some(Transaction::default());
    let descendant = runtime
        .instantiate_runtime_model("Task", &identity)
        .expect("dynamic child should be usable as an owner while live");
    let created = runtime
        .transaction
        .take()
        .expect("transaction should exist");
    runtime.commit(created);
    assert_eq!(
        runtime.dynamic_model_owners.get(&descendant),
        Some(&identity)
    );

    let error = run_test_transaction(&mut runtime, |runtime| {
        runtime.terminate_runtime_model(&identity, "left")
    })
    .expect_err("termination must not orphan a rooted descendant");

    assert!(error.message.contains("roots another live child"));
    assert!(runtime.model_identity_exists(&identity));
    assert!(runtime.model_identity_exists(&descendant));
}

// Contract: membership proves current structural occurrence, not rooting authority.
#[test]
fn foreign_membership_owner_is_not_lifetime_authority() {
    let mut runtime = runtime(OPTIONAL_SOURCE);
    runtime.run_action("seed").expect("seed should commit");
    let identity = targets(&mut runtime, "__meld_mseq$left$tasks")[0].clone();

    let error = run_test_transaction(&mut runtime, |runtime| {
        runtime.terminate_runtime_model(&identity, "right")
    })
    .expect_err("foreign membership owner must not gain lifetime authority");

    assert!(error.message.contains("requires rooting owner 'left'"));
    assert!(error.message.contains("not 'right'"));
    assert!(runtime.model_identity_exists(&identity));
    assert_eq!(
        targets(&mut runtime, "__meld_mseq$left$tasks"),
        vec![identity.clone()]
    );
    assert_eq!(
        targets(&mut runtime, "__meld_mseq$right$tasks"),
        vec![identity.clone()]
    );
    assert_eq!(
        runtime.value("__meld_live$selected").unwrap(),
        Value::String(identity)
    );
}
