from pathlib import Path


def replace_once(path: Path, old: str, new: str, label: str) -> None:
    text = path.read_text()
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{path}: {label}: expected one match, found {count}")
    path.write_text(text.replace(old, new, 1))

runtime = Path("compiler/src/runtime.rs")
old = '''        self.transaction
            .as_mut()
            .expect("transaction should exist while transferring model provenance")
            .updated_model_owners
            .insert(identity.to_string(), destination_owner.to_string());
        Ok(())
    }

    fn terminate_runtime_model(
'''
new = '''        self.transaction
            .as_mut()
            .expect("transaction should exist while transferring model provenance")
            .updated_model_owners
            .insert(identity.to_string(), destination_owner.to_string());
        Ok(())
    }

    #[cfg(test)]
    fn transfer_destination_would_create_provenance_cycle(
        &self,
        identity: &str,
        destination_owner: &str,
    ) -> bool {
        let mut current = destination_owner.to_string();
        let mut visited = HashSet::new();

        loop {
            if current == identity {
                return true;
            }
            if !visited.insert(current.clone()) {
                return true;
            }
            let Some(owner) = self.current_dynamic_model_owner(&current) else {
                return false;
            };
            current = owner;
        }
    }

    #[cfg(test)]
    fn transfer_runtime_model_owner_nonleaf_experiment(
        &mut self,
        identity: &str,
        expected_owner: &str,
        destination_owner: &str,
    ) -> Result<(), RuntimeError> {
        let Some(transaction) = self.transaction.as_ref() else {
            return Err(RuntimeError::new(
                "runtime model provenance can only transfer inside an active action transaction",
            ));
        };

        if transaction.terminated_model_identities.contains(identity) {
            return Err(RuntimeError::new(
                "runtime modeled-state child is already terminated in this transaction",
            ));
        }
        if !self.dynamic_model_owners.contains_key(identity) {
            return Err(RuntimeError::new(
                "runtime provenance transfer requires an existing committed dynamic child",
            ));
        }

        let actual_owner = self
            .current_dynamic_model_owner(identity)
            .expect("committed live dynamic child should have an owner");
        if actual_owner != expected_owner {
            return Err(RuntimeError::new(format!(
                "runtime modeled-state child is currently rooted in '{actual_owner}', not expected owner '{expected_owner}'"
            )));
        }

        if identity == destination_owner {
            return Err(RuntimeError::new(
                "runtime modeled-state child cannot become its own rooting owner",
            ));
        }
        if !self.model_identity_exists(destination_owner) {
            return Err(RuntimeError::new(format!(
                "unknown modeled-state destination owner '{destination_owner}'"
            )));
        }
        if self.transfer_destination_would_create_provenance_cycle(identity, destination_owner) {
            return Err(RuntimeError::new(
                "runtime modeled-state provenance transfer would create an owner cycle",
            ));
        }

        if actual_owner == destination_owner {
            return Ok(());
        }

        self.transaction
            .as_mut()
            .expect("transaction should exist while transferring model provenance")
            .updated_model_owners
            .insert(identity.to_string(), destination_owner.to_string());
        Ok(())
    }

    fn terminate_runtime_model(
'''
replace_once(runtime, old, new, "test-only non-leaf transfer entry")

tests = Path("compiler/src/runtime/provenance_transfer_execution.rs")
text = tests.read_text()
append = r'''

#[test]
fn nonleaf_experiment_changes_only_parent_edge_and_preserves_descendant_provenance() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("seed").expect("seed should commit");
    let identity = targets(&mut runtime, "__meld_mseq$left$documents")[0].clone();

    runtime.transaction = Some(Transaction::default());
    let descendant = runtime
        .instantiate_runtime_model("Document", &identity)
        .expect("transferred identity should be usable as a runtime owner");
    let created = runtime.transaction.take().expect("transaction should exist");
    runtime.commit(created);

    run_test_transaction(&mut runtime, |runtime| {
        runtime.transfer_runtime_model_owner_nonleaf_experiment(&identity, "left", "right")?;
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
fn nonleaf_experiment_rollback_restores_parent_edge_without_touching_descendant_edge() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("seed").expect("seed should commit");
    let identity = targets(&mut runtime, "__meld_mseq$left$documents")[0].clone();

    runtime.transaction = Some(Transaction::default());
    let descendant = runtime
        .instantiate_runtime_model("Document", &identity)
        .expect("transferred identity should be usable as a runtime owner");
    let created = runtime.transaction.take().expect("transaction should exist");
    runtime.commit(created);

    runtime.transaction = Some(Transaction::default());
    runtime
        .transfer_runtime_model_owner_nonleaf_experiment(&identity, "left", "right")
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
fn nonleaf_experiment_rejects_destination_descendant_cycle() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("seed").expect("seed should commit");
    let identity = targets(&mut runtime, "__meld_mseq$left$documents")[0].clone();

    runtime.transaction = Some(Transaction::default());
    let descendant = runtime
        .instantiate_runtime_model("Document", &identity)
        .expect("transferred identity should be usable as a runtime owner");
    let created = runtime.transaction.take().expect("transaction should exist");
    runtime.commit(created);

    let error = run_test_transaction(&mut runtime, |runtime| {
        runtime.transfer_runtime_model_owner_nonleaf_experiment(&identity, "left", &descendant)
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
fn nonleaf_experiment_cycle_check_observes_staged_owner_changes() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("seed").expect("seed should commit");
    let identity = targets(&mut runtime, "__meld_mseq$left$documents")[0].clone();

    runtime.transaction = Some(Transaction::default());
    let candidate_destination = runtime
        .instantiate_runtime_model("Document", "right")
        .expect("candidate destination should be a committed dynamic identity");
    let created = runtime.transaction.take().expect("transaction should exist");
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
        .transfer_runtime_model_owner_nonleaf_experiment(
            &identity,
            "left",
            &candidate_destination,
        )
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
'''
if "nonleaf_experiment_changes_only_parent_edge" in text:
    raise SystemExit("experiment tests already present")
tests.write_text(text + append)
