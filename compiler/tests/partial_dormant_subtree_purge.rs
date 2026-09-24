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
state root: maybe live Node = none
state child: maybe live Node = none
state grandchild: maybe live Node = none
state outsider: maybe live Node = none
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
        insert childNode into right.children
    }
    child = root.children[0]

    create Node in child as grandchildNode {
        through grandchildNode.name = "Grandchild"
        insert grandchildNode into child.children
        insert grandchildNode into right.children
    }
    grandchild = child.children[0]

    create Node in right as outsiderNode {
        through outsiderNode.name = "Outsider"
        insert outsiderNode into right.children
    }
    outsider = right.children[2]
}

action purgeRoot {
    purge root in left
}

action proveRightOwnsOutsider {
    transfer outsider from right to right
}

action proveRootStillLive {
    transfer root from left to left
}

action selectFirstRight {
    outsider = right.children[0]
}

action selectSecondRight {
    outsider = right.children[1]
}

action readOutsiderName {
    observedName = outsider.name
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
fn committed_dormant_subtree_purges_without_materializing_doomed_payloads() {
    let checked = check_source_with_runtime_models(SOURCE)
        .expect("dormant subtree-purge pressure source should check");
    let mut initial = PartialPersistentRuntime::open(checked.clone(), MemoryProvider::default())
        .expect("fresh partial runtime should open");
    initial.run_action("seed").expect("seed should publish");

    let mut provider = initial.into_provider();
    provider.loads.clear();
    provider.replacements.clear();
    let backing_before = provider.backing.clone();
    assert_eq!(
        backing_before.len(),
        4,
        "seed should produce root, child, grandchild, and outsider backing"
    );

    let mut runtime = PartialPersistentRuntime::open(checked.clone(), provider)
        .expect("restart should leave all dynamic identities dormant");
    assert_eq!(runtime.dormant_backing_keys().len(), 4);

    runtime
        .run_action("purgeRoot")
        .expect("committed dormant provenance subtree should purge leaves-first");
    assert_eq!(
        runtime.dormant_backing_keys().len(),
        1,
        "only the outsider should remain as a live dormant dynamic identity"
    );

    let mut provider = runtime.into_provider();
    assert!(
        provider.loads.is_empty(),
        "purge should select the dormant subtree from provenance metadata without loading doomed payloads"
    );
    assert_eq!(provider.replacements.len(), 1);
    assert!(
        provider.replacements[0].is_empty(),
        "manifest publication should not rewrite dynamic backing when every referenced dormant payload is also terminated"
    );
    assert_eq!(
        provider.backing, backing_before,
        "physical opaque bytes may remain even after their identities leave the live manifest"
    );

    provider.loads.clear();
    provider.replacements.clear();
    let mut restarted = PartialPersistentRuntime::open(checked, provider)
        .expect("purged world should restart from the accepted manifest");
    assert_eq!(restarted.dormant_backing_keys().len(), 1);
    restarted
        .run_action("proveRightOwnsOutsider")
        .expect("surviving outsider should retain exact right-root provenance");
    restarted
        .run_action("proveRootStillLive")
        .expect_err("purged root designation must no longer denote a live identity");

    restarted
        .run_action("selectFirstRight")
        .expect("right membership should retain the outsider as its sole occurrence");
    let error = restarted
        .run_action("selectSecondRight")
        .expect_err("right membership should contain no second occurrence after purge cleanup");
    assert!(
        error.message.contains("out of bounds"),
        "right membership should contain exactly one surviving occurrence: {}",
        error.message
    );

    let provider = restarted.into_provider();
    assert!(
        provider.loads.is_empty(),
        "metadata proofs and static-root membership checks should not materialize outsider backing"
    );

    let mut restarted = PartialPersistentRuntime::open(
        check_source_with_runtime_models(SOURCE).expect("source should still check"),
        provider,
    )
    .expect("post-purge metadata checks should leave world restartable");
    restarted
        .materialize_designation("outsider")
        .expect("surviving outsider should remain independently materializable");
    restarted
        .run_action("readOutsiderName")
        .expect("surviving outsider payload should remain intact");
    assert_eq!(
        restarted.value("observedName").unwrap(),
        Value::String("Outsider".into())
    );
}
