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
    state children: [live Node] = []
}

state left: Node
state right: Node
state selected: maybe live Node = none
state survivor: maybe live Node = none
state pinned: live Node = live right
state observedName = ""

action seed {
    create Node in left as doomedNode {
        through doomedNode.name = "Doomed leaf"
        insert doomedNode into left.children
        selected = doomedNode
    }

    create Node in right as survivorNode {
        through survivorNode.name = "Survivor"
        insert survivorNode into right.children
        survivor = survivorNode
        pinned = survivorNode
    }
}

action reselectInThenDestroy {
    pinned = left.children[0]
    destroy selected in left
}

action proveLeftOwnsSelected {
    transfer selected from left to left
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
fn staged_plain_live_reselection_in_blocks_dormant_leaf_destroy_and_rolls_back() {
    let checked = check_source_with_runtime_models(SOURCE)
        .expect("dormant destroy plain-live reselection-in source should check");
    let mut initial = PartialPersistentRuntime::open(checked.clone(), MemoryProvider::default())
        .expect("fresh partial runtime should open");
    initial.run_action("seed").expect("seed should publish");

    let mut provider = initial.into_provider();
    provider.loads.clear();
    provider.replacements.clear();
    let manifest_before = provider.manifest.clone();
    let backing_before = provider.backing.clone();
    assert_eq!(
        backing_before.len(),
        2,
        "seed should produce doomed and survivor backing"
    );

    let mut runtime = PartialPersistentRuntime::open(checked.clone(), provider)
        .expect("restart should leave both dynamic identities dormant");
    assert_eq!(runtime.dormant_backing_keys().len(), 2);

    let error = runtime
        .run_action("reselectInThenDestroy")
        .expect_err("staged plain-live reselection into the target must block destroy");
    assert!(
        error.message.contains("plain live designation"),
        "failure should be the established plain-live blocker diagnostic: {}",
        error.message
    );
    assert_eq!(
        runtime.dormant_backing_keys().len(),
        2,
        "failed action must leave both identities live and dormant"
    );

    let provider = runtime.into_provider();
    assert_eq!(provider.manifest, manifest_before);
    assert_eq!(provider.backing, backing_before);
    assert!(
        provider.loads.is_empty(),
        "staged blocker should require no dynamic backing reads"
    );
    assert!(
        provider.replacements.is_empty(),
        "semantic failure must not attempt provider publication"
    );

    let mut proof_provider = provider.clone();
    proof_provider.loads.clear();
    proof_provider.replacements.clear();
    let mut proof = PartialPersistentRuntime::open(checked.clone(), proof_provider)
        .expect("failed action must leave the prior provenance world restartable");
    proof
        .run_action("proveLeftOwnsSelected")
        .expect("doomed leaf provenance must survive rollback");
    proof
        .run_action("proveRightOwnsSurvivor")
        .expect("survivor provenance must survive rollback");
    assert!(
        proof.into_provider().loads.is_empty(),
        "post-failure provenance proofs should remain metadata-only"
    );

    let mut provider = provider;
    provider.loads.clear();
    provider.replacements.clear();
    let mut restarted = PartialPersistentRuntime::open(checked, provider)
        .expect("failed action must leave committed designation state restartable");
    restarted
        .materialize_designation("pinned")
        .expect("rollback must restore pinned to the committed survivor target");
    restarted
        .run_action("readPinnedName")
        .expect("restored plain-live designation should read survivor payload");
    assert_eq!(
        restarted.value("observedName").unwrap(),
        Value::String("Survivor".into())
    );

    let provider = restarted.into_provider();
    assert_eq!(
        provider.loads.len(),
        1,
        "only later explicit survivor materialization should read backing"
    );
}
