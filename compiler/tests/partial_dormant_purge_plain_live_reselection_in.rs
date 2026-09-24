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
state fallback: Node
state root: maybe live Node = none
state child: maybe live Node = none
state grandchild: maybe live Node = none
state foreign: maybe live Node = none
state pinned: live Node = live fallback
state observedName = ""

action seed {
    create Node in right as foreignNode {
        through foreignNode.name = "Foreign survivor"
        insert foreignNode into right.children
    }
    foreign = right.children[0]
    pinned = right.children[0]

    create Node in left as rootNode {
        through rootNode.name = "Root"
        insert rootNode into left.children
    }
    root = left.children[0]

    create Node in root as childNode {
        through childNode.name = "Child"
        insert childNode into root.children
    }
    child = root.children[0]

    create Node in child as grandchildNode {
        through grandchildNode.name = "Grandchild"
        insert grandchildNode into child.children
        insert grandchildNode into right.children
    }
    grandchild = child.children[0]
}

action reselectInThenPurge {
    pinned = right.children[1]
    purge root in left
}

action proveLeftOwnsRoot {
    transfer root from left to left
}

action proveRootOwnsChild {
    transfer child from root to root
}

action proveChildOwnsGrandchild {
    transfer grandchild from child to child
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
fn staged_plain_live_reselection_in_blocks_dormant_subtree_purge_and_rolls_back() {
    let checked = check_source_with_runtime_models(SOURCE)
        .expect("dormant purge plain-live reselection-in source should check");
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
        4,
        "seed should produce root, child, grandchild, and foreign backing"
    );

    let mut runtime = PartialPersistentRuntime::open(checked.clone(), provider)
        .expect("restart should leave all four dynamic identities dormant");
    assert_eq!(runtime.dormant_backing_keys().len(), 4);

    let error = runtime
        .run_action("reselectInThenPurge")
        .expect_err("staged plain-live reselection into the subtree must block purge");
    assert!(
        error.message.contains("plain live designation"),
        "failure should be the established plain-live blocker diagnostic: {}",
        error.message
    );
    assert_eq!(
        runtime.dormant_backing_keys().len(),
        4,
        "failed action must leave every dynamic identity live and dormant"
    );

    let provider = runtime.into_provider();
    assert_eq!(provider.manifest, manifest_before);
    assert_eq!(provider.backing, backing_before);
    assert!(
        provider.loads.is_empty(),
        "staged plain-live blocker should require no dynamic backing reads"
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
        .run_action("proveLeftOwnsRoot")
        .expect("root provenance must survive rollback");
    proof
        .run_action("proveRootOwnsChild")
        .expect("child provenance must survive rollback");
    proof
        .run_action("proveChildOwnsGrandchild")
        .expect("grandchild provenance must survive rollback");
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
        .expect("rollback must restore pinned to the committed foreign target");
    restarted
        .run_action("readPinnedName")
        .expect("restored plain-live designation should read the foreign target");
    assert_eq!(
        restarted.value("observedName").unwrap(),
        Value::String("Foreign survivor".into())
    );

    let provider = restarted.into_provider();
    assert_eq!(
        provider.loads.len(),
        1,
        "only later explicit materialization of the restored pinned target should read backing"
    );
}
