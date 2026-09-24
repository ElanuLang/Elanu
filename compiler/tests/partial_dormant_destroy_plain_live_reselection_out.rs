use std::collections::HashMap;

use elanu_compiler::{
    check_source_with_runtime_models,
    runtime::{
        persistence::partial::{PartialPersistenceProvider, PartialPersistentRuntime},
        RuntimeError, Value,
    },
};

const SOURCE: &str = r#"
state model Node {
    state name = ""
}

state left: Node
state right: Node
state selected: maybe live Node = none
state survivor: maybe live Node = none
state pinned: live Node = live left
state observedName = ""

action seed {
    create Node in left as doomedNode {
        through doomedNode.name = "Doomed leaf"
        insert doomedNode into left.children
        selected = doomedNode
        pinned = doomedNode
    }

    create Node in right as survivorNode {
        through survivorNode.name = "Survivor"
        insert survivorNode into right.children
        survivor = survivorNode
    }
}

action reselectOutThenDestroy {
    pinned = right.children[0]
    destroy selected in left
}

action proveRightOwnsSurvivor {
    transfer survivor from right to right
}

action readPinnedName {
    observedName = pinned.name
}
"#;

#[derive(Debug, Clone, Default)]
struct MemoryProvider {
    manifest: Option<Vec<u8>>,
    backing: HashMap<Vec<u8>, Vec<u8>>,
    loads: Vec<Vec<u8>>,
    replacements: Vec<Vec<Vec<u8>>>,
}

impl PartialPersistenceProvider for MemoryProvider {
    fn load_manifest(&mut self) -> Result<Option<Vec<u8>>, RuntimeError> {
        Ok(self.manifest.clone())
    }

    fn load_backing(&mut self, key: &[u8]) -> Result<Option<Vec<u8>>, RuntimeError> {
        self.loads.push(key.to_vec());
        Ok(self.backing.get(key).cloned())
    }

    fn replace_candidate(
        &mut self,
        manifest: &[u8],
        backing_replacements: &[(Vec<u8>, Vec<u8>)],
    ) -> Result<(), RuntimeError> {
        let mut candidate = self.backing.clone();
        for (key, payload) in backing_replacements {
            candidate.insert(key.clone(), payload.clone());
        }
        self.backing = candidate;
        self.manifest = Some(manifest.to_vec());
        self.replacements.push(
            backing_replacements
                .iter()
                .map(|(key, _)| key.clone())
                .collect(),
        );
        Ok(())
    }
}

#[test]
fn staged_plain_live_reselection_out_allows_dormant_leaf_destroy_without_loading_targets() {
    let checked = check_source_with_runtime_models(SOURCE)
        .expect("dormant destroy plain-live reselection-out source should check");
    let mut initial = PartialPersistentRuntime::open(checked.clone(), MemoryProvider::default())
        .expect("fresh partial runtime should open");
    initial.run_action("seed").expect("seed should publish");

    let mut provider = initial.into_provider();
    provider.loads.clear();
    provider.replacements.clear();
    assert_eq!(
        provider.backing.len(),
        2,
        "seed should produce doomed and survivor backing"
    );

    let mut runtime = PartialPersistentRuntime::open(checked.clone(), provider)
        .expect("restart should leave both dynamic identities dormant");
    assert_eq!(runtime.dormant_backing_keys().len(), 2);

    runtime
        .run_action("reselectOutThenDestroy")
        .expect("staged plain-live reselection out should remove the destroy blocker");
    assert_eq!(
        runtime.dormant_backing_keys().len(),
        1,
        "destroy should retire only the doomed identity"
    );

    let provider = runtime.into_provider();
    assert!(
        provider.loads.is_empty(),
        "designation validation and dormant destroy should require no dynamic backing reads"
    );
    assert_eq!(
        provider.replacements.last().map(Vec::as_slice),
        Some([].as_slice()),
        "accepted publication should not rewrite surviving dynamic backing"
    );

    let mut proof_provider = provider.clone();
    proof_provider.loads.clear();
    proof_provider.replacements.clear();
    let mut proof = PartialPersistentRuntime::open(checked.clone(), proof_provider)
        .expect("accepted world should restart with survivor dormant");
    proof
        .run_action("proveRightOwnsSurvivor")
        .expect("right must still own the survivor from metadata alone");
    proof
        .materialize_designation("selected")
        .expect_err("destroyed selected designation must be cleared");
    assert!(
        proof.into_provider().loads.is_empty(),
        "survivor provenance and cleared selected designation should remain metadata-only"
    );

    let mut provider = provider;
    provider.loads.clear();
    provider.replacements.clear();
    let mut restarted = PartialPersistentRuntime::open(checked, provider)
        .expect("accepted world should preserve staged pinned reselection");
    restarted
        .materialize_designation("pinned")
        .expect("pinned should now target the surviving dormant identity");
    restarted
        .run_action("readPinnedName")
        .expect("reselected plain-live designation should read survivor payload");
    assert_eq!(
        restarted.value("observedName").unwrap(),
        Value::String("Survivor".into())
    );

    let provider = restarted.into_provider();
    assert_eq!(
        provider.loads.len(),
        1,
        "only later explicit materialization of pinned survivor should read backing"
    );
}
