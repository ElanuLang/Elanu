use super::*;
use crate::check_source_with_runtime_models;

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
state model Folder {
    state designationCarrier = ""
    state children: [live Folder] = []
}

state workspace: Folder
state holderA: maybe live Folder = none
state holderB: maybe live Folder = none
state target: maybe live Folder = none

action seed {
    create Folder in workspace as a {
        insert a into workspace.children
    }
    create Folder in workspace as b {
        insert b into workspace.children
    }
    create Folder in workspace as t {
        insert t into workspace.children
    }

    holderA = workspace.children[0]
    holderB = workspace.children[1]
    target = workspace.children[2]
}
"#;

fn register_model_local_maybe_designation(
    runtime: &mut Runtime,
    owner_identity: &str,
    member: &str,
    target_model: &str,
) -> String {
    let state_name = model_binding_name(owner_identity, member);
    assert!(runtime.states.contains_key(&state_name));
    runtime.runtime_designations.insert(
        state_name.clone(),
        RuntimeDesignationMetadata {
            model_name: target_model.to_string(),
            allows_none: true,
        },
    );
    state_name
}

#[test]
fn per_identity_model_member_state_can_reuse_existing_optional_designation_cleanup() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("seed").expect("seed should commit");

    let holders = targets(&mut runtime, "__meld_mseq$workspace$children");
    let holder_a = holders[0].clone();
    let holder_b = holders[1].clone();
    let target = holders[2].clone();

    let slot_a = register_model_local_maybe_designation(
        &mut runtime,
        &holder_a,
        "designationCarrier",
        "Folder",
    );
    let slot_b = register_model_local_maybe_designation(
        &mut runtime,
        &holder_b,
        "designationCarrier",
        "Folder",
    );

    assert_ne!(
        slot_a, slot_b,
        "each modeled identity must own a distinct slot"
    );
    assert_eq!(
        runtime.value(&slot_a).unwrap(),
        Value::String(String::new())
    );
    assert_eq!(
        runtime.value(&slot_b).unwrap(),
        Value::String(String::new())
    );

    run_test_transaction(&mut runtime, |runtime| {
        runtime.write_state(&slot_a, Value::String(target.clone()))
    })
    .expect("first model-local optional designation should select the target");

    assert_eq!(
        runtime.value(&slot_a).unwrap(),
        Value::String(target.clone())
    );
    assert_eq!(
        runtime.value(&slot_b).unwrap(),
        Value::String(String::new())
    );
    assert_eq!(
        runtime.current_dynamic_model_owner(&target).as_deref(),
        Some("workspace")
    );

    run_test_transaction(&mut runtime, |runtime| {
        runtime.write_state(&slot_b, Value::String(target.clone()))
    })
    .expect("second modeled identity should independently select the same target");

    run_test_transaction(&mut runtime, |runtime| {
        runtime.terminate_runtime_model(&target, "workspace")
    })
    .expect("existing lifetime cleanup should clear model-local optional designations");

    assert_eq!(
        runtime.value(&slot_a).unwrap(),
        Value::String(String::new())
    );
    assert_eq!(
        runtime.value(&slot_b).unwrap(),
        Value::String(String::new())
    );
    assert!(!runtime.model_identity_exists(&target));
    assert!(runtime.model_identity_exists(&holder_a));
    assert!(runtime.model_identity_exists(&holder_b));
}

#[test]
fn aborted_target_termination_restores_model_local_optional_designation_slots() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("seed").expect("seed should commit");

    let identities = targets(&mut runtime, "__meld_mseq$workspace$children");
    let holder_a = identities[0].clone();
    let holder_b = identities[1].clone();
    let target = identities[2].clone();

    let slot_a = register_model_local_maybe_designation(
        &mut runtime,
        &holder_a,
        "designationCarrier",
        "Folder",
    );
    let slot_b = register_model_local_maybe_designation(
        &mut runtime,
        &holder_b,
        "designationCarrier",
        "Folder",
    );

    run_test_transaction(&mut runtime, |runtime| {
        runtime.write_state(&slot_a, Value::String(target.clone()))?;
        runtime.write_state(&slot_b, Value::String(target.clone()))
    })
    .expect("model-local designation selections should commit");

    runtime.transaction = Some(Transaction::default());
    runtime
        .terminate_runtime_model(&target, "workspace")
        .expect("termination should stage cleanup");
    assert_eq!(
        runtime.value(&slot_a).unwrap(),
        Value::String(String::new())
    );
    assert_eq!(
        runtime.value(&slot_b).unwrap(),
        Value::String(String::new())
    );
    runtime.transaction = None;

    assert!(runtime.model_identity_exists(&target));
    assert_eq!(
        runtime.value(&slot_a).unwrap(),
        Value::String(target.clone())
    );
    assert_eq!(runtime.value(&slot_b).unwrap(), Value::String(target));
}
