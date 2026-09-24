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
state fallback: Node
state root: maybe live Node = none
state child: maybe live Node = none
state grandchild: maybe live Node = none
state pinned: live Node = live fallback
state observedName = ""

action seed {
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
    }
    grandchild = child.children[0]
    pinned = child.children[0]
}

action purgeRoot {
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

action readGrandchildName {
    observedName = grandchild.name
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
fn plain_live_designation_blocks_dormant_subtree_purge_without_payload_reads() {
    let checked = check_source_with_runtime_models(SOURCE)
        .expect("dormant purge plain-live blocker source should check");
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
        3,
        "seed should produce root, child, and grandchild backing"
    );

    let mut runtime = PartialPersistentRuntime::open(checked.clone(), provider)
        .expect("restart should leave the complete dynamic subtree dormant");
    assert_eq!(runtime.dormant_backing_keys().len(), 3);

    let error = runtime
        .run_action("purgeRoot")
        .expect_err("plain live designation into the subtree must block purge");
    assert!(
        error.message.contains("plain live designation"),
        "failure should be the established plain-live blocker diagnostic: {}",
        error.message
    );
    assert_eq!(
        runtime.dormant_backing_keys().len(),
        3,
        "blocked purge must leave every dynamic identity live and dormant"
    );

    let provider = runtime.into_provider();
    assert_eq!(provider.manifest, manifest_before);
    assert_eq!(provider.backing, backing_before);
    assert!(
        provider.loads.is_empty(),
        "plain-live blocking should use persistent designation/provenance metadata without payload reads"
    );
    assert!(
        provider.replacements.is_empty(),
        "semantic purge failure must not attempt durable publication"
    );

    let mut restarted = PartialPersistentRuntime::open(checked.clone(), provider)
        .expect("blocked purge must leave the prior durable world restartable");
    restarted
        .run_action("proveLeftOwnsRoot")
        .expect("blocked purge must preserve root provenance");
    restarted
        .run_action("proveRootOwnsChild")
        .expect("blocked purge must preserve child provenance");
    restarted
        .run_action("proveChildOwnsGrandchild")
        .expect("blocked purge must preserve grandchild provenance");

    let mut provider = restarted.into_provider();
    assert!(
        provider.loads.is_empty(),
        "post-failure provenance proofs should remain metadata-only"
    );
    provider.loads.clear();
    provider.replacements.clear();

    let mut restarted = PartialPersistentRuntime::open(checked, provider)
        .expect("metadata proofs should leave the blocked world restartable");
    restarted
        .materialize_designation("grandchild")
        .expect("pinned dormant grandchild should remain explicitly materializable");
    restarted
        .run_action("readGrandchildName")
        .expect("blocked purge must preserve grandchild payload");
    assert_eq!(
        restarted.value("observedName").unwrap(),
        Value::String("Grandchild".into())
    );

    let provider = restarted.into_provider();
    assert_eq!(
        provider.loads.len(),
        1,
        "only explicit later grandchild materialization should read backing"
    );
}
