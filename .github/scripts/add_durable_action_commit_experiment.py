from pathlib import Path

path = Path("compiler/src/runtime/restart_checkpoint_experiment.rs")
text = path.read_text()
anchor = "\n#[test]\nfn checkpoint_rejects_active_transaction_and_does_not_capture_staged_work() {\n"
if text.count(anchor) != 1:
    raise SystemExit(f"expected one insertion anchor, found {text.count(anchor)}")

addition = r'''
fn run_action_with_durable_acceptance(
    runtime: &mut Runtime,
    checked: &crate::CheckedSource,
    name: &str,
    mut accept: impl FnMut(&RuntimeCheckpoint) -> Result<(), RuntimeError>,
) -> Result<(), RuntimeError> {
    if runtime.transaction.is_some() {
        return Err(RuntimeError::new(
            "durable action cannot start while a transaction is active",
        ));
    }

    let prior = runtime.capture_restart_checkpoint()?;
    runtime.transaction = Some(Transaction::default());
    let result = runtime.invoke_action(name, &[]);
    let transaction = match result {
        Ok(()) => runtime
            .transaction
            .take()
            .expect("successful durable action should retain its transaction"),
        Err(error) => {
            runtime.transaction = None;
            runtime.next_dynamic_identity = prior.next_dynamic_identity;
            return Err(error);
        }
    };

    let candidate_next_dynamic_identity = runtime.next_dynamic_identity;
    let mut candidate = Runtime::from_checked_source(checked)?;
    candidate.restore_restart_checkpoint(&prior)?;
    candidate.next_dynamic_identity = candidate_next_dynamic_identity;
    candidate.commit(transaction);
    let candidate_checkpoint = candidate.capture_restart_checkpoint()?;

    if let Err(error) = accept(&candidate_checkpoint) {
        runtime.next_dynamic_identity = prior.next_dynamic_identity;
        return Err(error);
    }

    *runtime = candidate;
    Ok(())
}

#[test]
fn durable_rejection_preserves_the_prior_committed_world_exactly() {
    let checked = checked_source();
    let mut runtime = Runtime::from_checked_source(&checked).expect("runtime should initialize");
    seed_and_move_to_trash(&mut runtime);
    let prior = runtime
        .capture_restart_checkpoint()
        .expect("prior committed world should checkpoint");

    let error = run_action_with_durable_acceptance(
        &mut runtime,
        &checked,
        "restoreFromTrash",
        |_| Err(RuntimeError::new("durable provider rejected candidate")),
    )
    .expect_err("provider rejection must fail the action");
    assert!(error.message.contains("durable provider rejected"));

    let after = runtime
        .capture_restart_checkpoint()
        .expect("runtime should remain committed after rejection");
    assert_eq!(after, prior);
    assert_eq!(runtime.value("selectedInTrash").unwrap(), Value::Bool(true));
    assert_eq!(runtime.value("selectedInSource").unwrap(), Value::Bool(false));
}

#[test]
fn durable_acceptance_publishes_exactly_the_accepted_candidate() {
    let checked = checked_source();
    let mut runtime = Runtime::from_checked_source(&checked).expect("runtime should initialize");
    seed_and_move_to_trash(&mut runtime);
    let mut accepted = None;

    run_action_with_durable_acceptance(
        &mut runtime,
        &checked,
        "restoreFromTrash",
        |checkpoint| {
            accepted = Some(checkpoint.clone());
            Ok(())
        },
    )
    .expect("provider acceptance should publish the candidate world");

    let current = runtime
        .capture_restart_checkpoint()
        .expect("published world should checkpoint");
    assert_eq!(Some(current), accepted);
    assert_eq!(runtime.value("selectedInSource").unwrap(), Value::Bool(true));
    assert_eq!(runtime.value("selectedInTrash").unwrap(), Value::Bool(false));
    runtime
        .run_action("currentOwnerProof")
        .expect("published candidate must include transferred provenance");
}

#[test]
fn semantic_failure_never_attempts_durable_acceptance() {
    let checked = checked_source();
    let mut runtime = Runtime::from_checked_source(&checked).expect("runtime should initialize");
    seed_and_move_to_trash(&mut runtime);
    let prior = runtime
        .capture_restart_checkpoint()
        .expect("prior committed world should checkpoint");
    let mut attempts = 0;

    run_action_with_durable_acceptance(&mut runtime, &checked, "failedRename", |_| {
        attempts += 1;
        Ok(())
    })
    .expect_err("semantic failure must abort before durable acceptance");

    assert_eq!(attempts, 0);
    assert_eq!(
        runtime.capture_restart_checkpoint().unwrap(),
        prior,
        "semantic failure must preserve the prior durable world",
    );
}

#[test]
fn rejected_dynamic_creation_does_not_publish_or_consume_restart_identity() {
    let checked = checked_source();
    let mut runtime = Runtime::from_checked_source(&checked).expect("runtime should initialize");
    seed_and_move_to_trash(&mut runtime);
    let prior = runtime
        .capture_restart_checkpoint()
        .expect("prior committed world should checkpoint");

    run_action_with_durable_acceptance(
        &mut runtime,
        &checked,
        "createAnotherFolder",
        |_| Err(RuntimeError::new("storage unavailable")),
    )
    .expect_err("rejected candidate creation must fail");
    assert_eq!(runtime.capture_restart_checkpoint().unwrap(), prior);

    let existing_ids = runtime
        .dynamic_model_types
        .keys()
        .cloned()
        .collect::<HashSet<_>>();
    run_action_with_durable_acceptance(
        &mut runtime,
        &checked,
        "createAnotherFolder",
        |_| Ok(()),
    )
    .expect("accepted retry should commit");

    let new_ids = runtime
        .dynamic_model_types
        .keys()
        .filter(|identity| !existing_ids.contains(*identity))
        .collect::<Vec<_>>();
    assert_eq!(new_ids.len(), 1);
    assert_eq!(runtime.next_dynamic_identity, prior.next_dynamic_identity + 1);
}
'''

path.write_text(text.replace(anchor, "\n" + addition + anchor, 1))
