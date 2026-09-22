use super::*;
use crate::check_source_with_runtime_models;

// Runtime provenance regression coverage for the compiler-accepted transfer law.
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

const SOURCE: &str = r#"
state model Document {
    state title = ""
}

state model Folder {
    state documents: [live Document] = []
}

state left: Folder
state right: Folder
state selected: maybe live Document = none

action seed {
    create Document in left as document {
        through document.title = "Draft"
        insert document into left.documents
        insert document into right.documents
    }
    selected = left.documents[0]
}
"#;

#[test]
fn transfer_changes_only_root_provenance_and_preserves_identity_and_membership() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("seed").expect("seed should commit");

    let identity = targets(&mut runtime, "__meld_mseq$left$documents")[0].clone();
    assert_eq!(
        runtime.current_dynamic_model_owner(&identity).as_deref(),
        Some("left")
    );

    run_test_transaction(&mut runtime, |runtime| {
        runtime.transfer_runtime_model_owner(&identity, "left", "right")?;
        assert_eq!(
            runtime.current_dynamic_model_owner(&identity).as_deref(),
            Some("right")
        );
        assert_eq!(
            runtime.value("__meld_live$selected")?,
            Value::String(identity.clone())
        );
        assert_eq!(
            targets(runtime, "__meld_mseq$left$documents"),
            vec![identity.clone()]
        );
        assert_eq!(
            targets(runtime, "__meld_mseq$right$documents"),
            vec![identity.clone()]
        );
        Ok(())
    })
    .expect("leaf provenance transfer should commit");

    assert_eq!(
        runtime.dynamic_model_owners.get(&identity),
        Some(&"right".to_string())
    );
    assert_eq!(
        runtime.value("__meld_live$selected").unwrap(),
        Value::String(identity.clone())
    );
    assert_eq!(
        targets(&mut runtime, "__meld_mseq$left$documents"),
        vec![identity.clone()]
    );
    assert_eq!(
        targets(&mut runtime, "__meld_mseq$right$documents"),
        vec![identity]
    );
}

#[test]
fn aborted_transfer_restores_committed_provenance() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("seed").expect("seed should commit");
    let identity = targets(&mut runtime, "__meld_mseq$left$documents")[0].clone();

    runtime.transaction = Some(Transaction::default());
    runtime
        .transfer_runtime_model_owner(&identity, "left", "right")
        .expect("transfer should stage");
    assert_eq!(
        runtime.current_dynamic_model_owner(&identity).as_deref(),
        Some("right")
    );
    runtime.transaction = None;

    assert_eq!(
        runtime.current_dynamic_model_owner(&identity).as_deref(),
        Some("left")
    );
}

#[test]
fn staged_transfer_changes_rooting_proof_for_later_lifetime_work() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("seed").expect("seed should commit");
    let identity = targets(&mut runtime, "__meld_mseq$left$documents")[0].clone();

    runtime.transaction = Some(Transaction::default());
    runtime
        .transfer_runtime_model_owner(&identity, "left", "right")
        .expect("transfer should stage");

    let old_owner_error = runtime
        .terminate_runtime_model(&identity, "left")
        .expect_err("old owner must stop proving lifetime authority immediately");
    assert!(old_owner_error
        .message
        .contains("requires rooting owner 'right'"));

    runtime
        .terminate_runtime_model(&identity, "right")
        .expect("new owner should prove lifetime authority in the same transaction");
    assert!(!runtime.model_identity_exists(&identity));

    runtime.transaction = None;
    assert!(runtime.model_identity_exists(&identity));
    assert_eq!(
        runtime.current_dynamic_model_owner(&identity).as_deref(),
        Some("left")
    );
}

#[test]
fn transfer_requires_the_exact_current_source_owner() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("seed").expect("seed should commit");
    let identity = targets(&mut runtime, "__meld_mseq$left$documents")[0].clone();

    let error = run_test_transaction(&mut runtime, |runtime| {
        runtime.transfer_runtime_model_owner(&identity, "right", "left")
    })
    .expect_err("stale source-owner proof must fail");

    assert!(error.message.contains("currently rooted in 'left'"));
    assert_eq!(
        runtime.current_dynamic_model_owner(&identity).as_deref(),
        Some("left")
    );
}

#[test]
fn transfer_rejects_self_rooting_and_unknown_destination() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("seed").expect("seed should commit");
    let identity = targets(&mut runtime, "__meld_mseq$left$documents")[0].clone();

    let self_error = run_test_transaction(&mut runtime, |runtime| {
        runtime.transfer_runtime_model_owner(&identity, "left", &identity)
    })
    .expect_err("child cannot become its own lifetime root");
    assert!(self_error
        .message
        .contains("cannot become its own rooting owner"));

    let missing_error = run_test_transaction(&mut runtime, |runtime| {
        runtime.transfer_runtime_model_owner(&identity, "left", "missing")
    })
    .expect_err("destination owner must be live");
    assert!(missing_error
        .message
        .contains("unknown modeled-state destination owner"));
}

#[test]
fn same_owner_transfer_is_a_valid_no_op_after_provenance_validation() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("seed").expect("seed should commit");
    let identity = targets(&mut runtime, "__meld_mseq$left$documents")[0].clone();

    run_test_transaction(&mut runtime, |runtime| {
        runtime.transfer_runtime_model_owner(&identity, "left", "left")
    })
    .expect("same-owner transfer should validate and succeed");

    assert_eq!(
        runtime.current_dynamic_model_owner(&identity).as_deref(),
        Some("left")
    );
}

#[test]
fn nonleaf_transfer_changes_only_parent_edge_and_preserves_descendant_provenance() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("seed").expect("seed should commit");
    let identity = targets(&mut runtime, "__meld_mseq$left$documents")[0].clone();

    runtime.transaction = Some(Transaction::default());
    let descendant = runtime
        .instantiate_runtime_model("Document", &identity)
        .expect("transferred identity should be usable as a runtime owner");
    let created = runtime
        .transaction
        .take()
        .expect("transaction should exist");
    runtime.commit(created);

    run_test_transaction(&mut runtime, |runtime| {
        runtime.transfer_runtime_model_owner(&identity, "left", "right")?;
        assert_eq!(
            runtime.current_dynamic_model_owner(&identity).as_deref(),
            Some("right")
        );
        assert_eq!(
            runtime.current_dynamic_model_owner(&descendant).as_deref(),
            Some(identity.as_str())
        );
        Ok(())
    })
    .expect("acyclic non-leaf parent-edge transfer should commit");

    assert_eq!(
        runtime.current_dynamic_model_owner(&identity).as_deref(),
        Some("right")
    );
    assert_eq!(
        runtime.current_dynamic_model_owner(&descendant).as_deref(),
        Some(identity.as_str())
    );
}

#[test]
fn nonleaf_transfer_rollback_restores_parent_edge_without_touching_descendant_edge() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("seed").expect("seed should commit");
    let identity = targets(&mut runtime, "__meld_mseq$left$documents")[0].clone();

    runtime.transaction = Some(Transaction::default());
    let descendant = runtime
        .instantiate_runtime_model("Document", &identity)
        .expect("transferred identity should be usable as a runtime owner");
    let created = runtime
        .transaction
        .take()
        .expect("transaction should exist");
    runtime.commit(created);

    runtime.transaction = Some(Transaction::default());
    runtime
        .transfer_runtime_model_owner(&identity, "left", "right")
        .expect("acyclic non-leaf transfer should stage");
    assert_eq!(
        runtime.current_dynamic_model_owner(&identity).as_deref(),
        Some("right")
    );
    assert_eq!(
        runtime.current_dynamic_model_owner(&descendant).as_deref(),
        Some(identity.as_str())
    );
    runtime.transaction = None;

    assert_eq!(
        runtime.current_dynamic_model_owner(&identity).as_deref(),
        Some("left")
    );
    assert_eq!(
        runtime.current_dynamic_model_owner(&descendant).as_deref(),
        Some(identity.as_str())
    );
}

#[test]
fn nonleaf_transfer_rejects_destination_descendant_cycle() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("seed").expect("seed should commit");
    let identity = targets(&mut runtime, "__meld_mseq$left$documents")[0].clone();

    runtime.transaction = Some(Transaction::default());
    let descendant = runtime
        .instantiate_runtime_model("Document", &identity)
        .expect("transferred identity should be usable as a runtime owner");
    let created = runtime
        .transaction
        .take()
        .expect("transaction should exist");
    runtime.commit(created);

    let error = run_test_transaction(&mut runtime, |runtime| {
        runtime.transfer_runtime_model_owner(&identity, "left", &descendant)
    })
    .expect_err("destination descendant must be rejected to preserve acyclic provenance");

    assert!(error.message.contains("would create an owner cycle"));
    assert_eq!(
        runtime.current_dynamic_model_owner(&identity).as_deref(),
        Some("left")
    );
    assert_eq!(
        runtime.current_dynamic_model_owner(&descendant).as_deref(),
        Some(identity.as_str())
    );
}

#[test]
fn nonleaf_transfer_cycle_check_observes_staged_owner_changes() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("seed").expect("seed should commit");
    let identity = targets(&mut runtime, "__meld_mseq$left$documents")[0].clone();

    runtime.transaction = Some(Transaction::default());
    let candidate_destination = runtime
        .instantiate_runtime_model("Document", "right")
        .expect("candidate destination should be a committed dynamic identity");
    let created = runtime
        .transaction
        .take()
        .expect("transaction should exist");
    runtime.commit(created);

    runtime.transaction = Some(Transaction::default());
    runtime
        .transfer_runtime_model_owner(&candidate_destination, "right", &identity)
        .expect("leaf transfer should stage candidate beneath the identity");
    assert_eq!(
        runtime
            .current_dynamic_model_owner(&candidate_destination)
            .as_deref(),
        Some(identity.as_str())
    );

    let error = runtime
        .transfer_runtime_model_owner(&identity, "left", &candidate_destination)
        .expect_err("cycle check must observe the staged destination ancestry");
    assert!(error.message.contains("would create an owner cycle"));

    runtime.transaction = None;
    assert_eq!(
        runtime
            .current_dynamic_model_owner(&candidate_destination)
            .as_deref(),
        Some("right")
    );
}
