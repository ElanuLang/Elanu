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

const SUBTREE_PURGE_SOURCE: &str = r#"
state model Node {
    state children: [live Node] = []
}

state left: Node
state right: Node
state fallback: Node
state root: maybe live Node = none
state child: maybe live Node = none
state grandchild: maybe live Node = none
state outsider: maybe live Node = none
state required: live Node = live fallback

action seedSubtree {
    create Node in left as rootNode {
        insert rootNode into left.children
    }
    root = left.children[0]

    create Node in root as childNode {
        insert childNode into root.children
        insert childNode into right.children
    }
    child = root.children[0]

    create Node in child as grandchildNode {
        insert grandchildNode into child.children
        insert grandchildNode into right.children
    }
    grandchild = child.children[0]

    create Node in right as outsiderNode {
        insert outsiderNode into right.children
    }
    outsider = right.children[2]
}

action pinChild {
    required = root.children[0]
}
"#;

fn designation_target(runtime: &mut Runtime, name: &str) -> String {
    let Value::String(target) = runtime
        .value(&format!("__meld_live${name}"))
        .expect("designation should exist")
    else {
        panic!("designation should carry a String identity");
    };
    target
}

#[test]
fn subtree_purge_experiment_reuses_leaf_cleanup_for_entire_provenance_subtree() {
    let mut runtime = runtime(SUBTREE_PURGE_SOURCE);
    runtime
        .run_action("seedSubtree")
        .expect("subtree source should seed");

    let root = designation_target(&mut runtime, "root");
    let child = designation_target(&mut runtime, "child");
    let grandchild = designation_target(&mut runtime, "grandchild");
    let outsider = designation_target(&mut runtime, "outsider");

    run_test_transaction(&mut runtime, |runtime| {
        runtime.terminate_runtime_model_subtree_experiment(&root, "left")
    })
    .expect("committed provenance subtree should purge leaves-first");

    assert!(!runtime.model_identity_exists(&root));
    assert!(!runtime.model_identity_exists(&child));
    assert!(!runtime.model_identity_exists(&grandchild));
    assert!(runtime.model_identity_exists(&outsider));
    assert!(targets(&mut runtime, "__meld_mseq$left$children").is_empty());
    assert_eq!(
        targets(&mut runtime, "__meld_mseq$right$children"),
        vec![outsider]
    );
    assert_eq!(
        runtime.value("__meld_live$root").unwrap(),
        Value::String(String::new())
    );
    assert_eq!(
        runtime.value("__meld_live$child").unwrap(),
        Value::String(String::new())
    );
    assert_eq!(
        runtime.value("__meld_live$grandchild").unwrap(),
        Value::String(String::new())
    );
}

#[test]
fn subtree_purge_experiment_rolls_back_the_complete_subtree() {
    let mut runtime = runtime(SUBTREE_PURGE_SOURCE);
    runtime
        .run_action("seedSubtree")
        .expect("subtree source should seed");

    let root = designation_target(&mut runtime, "root");
    let child = designation_target(&mut runtime, "child");
    let grandchild = designation_target(&mut runtime, "grandchild");

    runtime.transaction = Some(Transaction::default());
    runtime
        .terminate_runtime_model_subtree_experiment(&root, "left")
        .expect("subtree purge should stage");
    assert!(!runtime.model_identity_exists(&root));
    assert!(!runtime.model_identity_exists(&child));
    assert!(!runtime.model_identity_exists(&grandchild));
    runtime.transaction = None;

    assert!(runtime.model_identity_exists(&root));
    assert!(runtime.model_identity_exists(&child));
    assert!(runtime.model_identity_exists(&grandchild));
    assert_eq!(designation_target(&mut runtime, "root"), root);
    assert_eq!(designation_target(&mut runtime, "child"), child);
    assert_eq!(designation_target(&mut runtime, "grandchild"), grandchild);
}

#[test]
fn subtree_purge_experiment_plain_live_descendant_blocks_and_rolls_back() {
    let mut runtime = runtime(SUBTREE_PURGE_SOURCE);
    runtime
        .run_action("seedSubtree")
        .expect("subtree source should seed");
    runtime
        .run_action("pinChild")
        .expect("plain live child designation should pin");

    let root = designation_target(&mut runtime, "root");
    let child = designation_target(&mut runtime, "child");
    let grandchild = designation_target(&mut runtime, "grandchild");

    let error = run_test_transaction(&mut runtime, |runtime| {
        runtime.terminate_runtime_model_subtree_experiment(&root, "left")
    })
    .expect_err("plain live descendant designation must block the whole purge");

    assert!(error.message.contains("plain live designation"));
    assert!(runtime.model_identity_exists(&root));
    assert!(runtime.model_identity_exists(&child));
    assert!(runtime.model_identity_exists(&grandchild));
    assert_eq!(designation_target(&mut runtime, "grandchild"), grandchild);
}

#[test]
fn subtree_purge_experiment_observes_staged_transfer_out() {
    let mut runtime = runtime(SUBTREE_PURGE_SOURCE);
    runtime
        .run_action("seedSubtree")
        .expect("subtree source should seed");

    let root = designation_target(&mut runtime, "root");
    let child = designation_target(&mut runtime, "child");
    let grandchild = designation_target(&mut runtime, "grandchild");

    run_test_transaction(&mut runtime, |runtime| {
        runtime.transfer_runtime_model_owner(&child, &root, "right")?;
        runtime.terminate_runtime_model_subtree_experiment(&root, "left")
    })
    .expect("child transferred out before purge should survive with its descendants");

    assert!(!runtime.model_identity_exists(&root));
    assert!(runtime.model_identity_exists(&child));
    assert!(runtime.model_identity_exists(&grandchild));
    assert_eq!(
        runtime.current_dynamic_model_owner(&child).as_deref(),
        Some("right")
    );
    assert_eq!(
        runtime.current_dynamic_model_owner(&grandchild).as_deref(),
        Some(child.as_str())
    );
}

#[test]
fn subtree_purge_experiment_observes_staged_transfer_in() {
    let mut runtime = runtime(SUBTREE_PURGE_SOURCE);
    runtime
        .run_action("seedSubtree")
        .expect("subtree source should seed");

    let root = designation_target(&mut runtime, "root");
    let outsider = designation_target(&mut runtime, "outsider");

    run_test_transaction(&mut runtime, |runtime| {
        runtime.transfer_runtime_model_owner(&outsider, "right", &root)?;
        runtime.terminate_runtime_model_subtree_experiment(&root, "left")
    })
    .expect("committed child transferred into subtree should join the purge");

    assert!(!runtime.model_identity_exists(&root));
    assert!(!runtime.model_identity_exists(&outsider));
    assert_eq!(
        runtime.value("__meld_live$outsider").unwrap(),
        Value::String(String::new())
    );
}

#[test]
fn subtree_purge_experiment_rejects_fresh_transaction_local_descendant() {
    let mut runtime = runtime(SUBTREE_PURGE_SOURCE);
    runtime
        .run_action("seedSubtree")
        .expect("subtree source should seed");

    let root = designation_target(&mut runtime, "root");
    runtime.transaction = Some(Transaction::default());
    let fresh = runtime
        .instantiate_runtime_model("Node", &root)
        .expect("fresh descendant should instantiate");

    let error = runtime
        .terminate_runtime_model_subtree_experiment(&root, "left")
        .expect_err("fresh descendant cancellation remains unselected");
    assert!(error
        .message
        .contains("fresh transaction-local descendants"));
    assert!(runtime.model_identity_exists(&root));
    assert!(runtime.model_identity_exists(&fresh));

    runtime.transaction = None;
    assert!(runtime.model_identity_exists(&root));
    assert!(!runtime.model_identity_exists(&fresh));
}
