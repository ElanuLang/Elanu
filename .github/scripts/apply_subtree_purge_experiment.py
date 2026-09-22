from pathlib import Path


def replace_once(path: Path, old: str, new: str, label: str) -> None:
    text = path.read_text()
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{path}: {label}: expected one match, found {count}")
    path.write_text(text.replace(old, new, 1))


runtime = Path("compiler/src/runtime.rs")
old = '''    fn terminate_runtime_model(
        &mut self,
        identity: &str,
        claimed_owner: &str,
    ) -> Result<(), RuntimeError> {
'''
new = '''    #[cfg(test)]
    fn committed_provenance_subtree_postorder_experiment(
        &self,
        root: &str,
    ) -> Result<Vec<String>, RuntimeError> {
        if !self.dynamic_model_owners.contains_key(root) {
            return Err(RuntimeError::new(
                "runtime subtree purge requires an existing committed dynamic root",
            ));
        }

        if let Some(transaction) = self.transaction.as_ref() {
            for fresh in transaction.created_model_owners.keys() {
                if transaction.terminated_model_identities.contains(fresh) {
                    continue;
                }
                let mut current = fresh.clone();
                let mut visited = HashSet::new();
                loop {
                    if !visited.insert(current.clone()) {
                        break;
                    }
                    let Some(owner) = self.current_dynamic_model_owner(&current) else {
                        break;
                    };
                    if owner == root {
                        return Err(RuntimeError::new(
                            "runtime subtree purge does not yet cancel fresh transaction-local descendants",
                        ));
                    }
                    current = owner;
                }
            }
        }

        fn visit(
            runtime: &Runtime,
            identity: &str,
            visited: &mut HashSet<String>,
            postorder: &mut Vec<String>,
        ) {
            if !visited.insert(identity.to_string()) {
                return;
            }
            let children = runtime
                .dynamic_model_owners
                .keys()
                .filter(|child| {
                    runtime.current_dynamic_model_owner(child).as_deref() == Some(identity)
                })
                .cloned()
                .collect::<Vec<_>>();
            for child in children {
                visit(runtime, &child, visited, postorder);
            }
            postorder.push(identity.to_string());
        }

        let mut visited = HashSet::new();
        let mut postorder = Vec::new();
        visit(self, root, &mut visited, &mut postorder);
        Ok(postorder)
    }

    #[cfg(test)]
    fn terminate_runtime_model_subtree_experiment(
        &mut self,
        identity: &str,
        claimed_owner: &str,
    ) -> Result<(), RuntimeError> {
        let actual_owner = self.current_dynamic_model_owner(identity).ok_or_else(|| {
            RuntimeError::new(
                "runtime subtree purge requires an existing live committed dynamic root",
            )
        })?;
        if actual_owner != claimed_owner {
            return Err(RuntimeError::new(format!(
                "runtime subtree purge requires rooting owner '{actual_owner}', not '{claimed_owner}'"
            )));
        }

        let postorder = self.committed_provenance_subtree_postorder_experiment(identity)?;
        for target in postorder {
            let owner = self.current_dynamic_model_owner(&target).ok_or_else(|| {
                RuntimeError::new(format!(
                    "runtime subtree purge lost current owner for '{target}'"
                ))
            })?;
            self.terminate_runtime_model(&target, &owner)?;
        }
        Ok(())
    }

    fn terminate_runtime_model(
        &mut self,
        identity: &str,
        claimed_owner: &str,
    ) -> Result<(), RuntimeError> {
'''
replace_once(runtime, old, new, "test-only subtree purge helpers")


tests = Path("compiler/src/runtime/lifetime_termination_execution.rs")
text = tests.read_text()
append = r'''

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
    required = child
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
    assert!(error.message.contains("fresh transaction-local descendants"));
    assert!(runtime.model_identity_exists(&root));
    assert!(runtime.model_identity_exists(&fresh));

    runtime.transaction = None;
    assert!(runtime.model_identity_exists(&root));
    assert!(!runtime.model_identity_exists(&fresh));
}
'''
if "subtree_purge_experiment_reuses_leaf_cleanup" in text:
    raise SystemExit("subtree purge experiment already present")
tests.write_text(text + append)
