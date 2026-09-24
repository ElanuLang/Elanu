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
state foreign: maybe live Node = none
state observedName = ""

action seed {
    create Node in left as rootNode {
        through rootNode.name = "Root"
        insert rootNode into left.children
    }
    root = left.children[0]

    create Node in right as foreignNode {
        through foreignNode.name = "Foreign survivor"
        insert foreignNode into right.children
    }
    foreign = right.children[0]

    create Node in root as childNode {
        through childNode.name = "Child"
        insert childNode into root.children
        insert childNode into foreign.children
    }
    child = root.children[0]

    create Node in child as grandchildNode {
        through grandchildNode.name = "Grandchild"
        insert grandchildNode into child.children
        insert grandchildNode into foreign.children
    }
    grandchild = child.children[0]
}

action purgeRoot {
    purge root in left
}

action proveRightOwnsForeign {
    transfer foreign from right to right
}

action selectForeignChild {
    child = foreign.children[0]
}

action readForeignName {
    observedName = foreign.name
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
fn purge_rewrites_only_surviving_dormant_foreign_owner_backing() {
    let checked = check_source_with_runtime_models(SOURCE)
        .expect("dormant purge foreign-cleanup pressure source should check");
    let mut initial = PartialPersistentRuntime::open(checked.clone(), MemoryProvider::default())
        .expect("fresh partial runtime should open");
    initial.run_action("seed").expect("seed should publish");

    let mut provider = initial.into_provider();
    provider.loads.clear();
    provider.replacements.clear();
    assert_eq!(
        provider.backing.len(),
        4,
        "seed should produce root, foreign survivor, child, and grandchild backing"
    );

    let mut runtime = PartialPersistentRuntime::open(checked.clone(), provider)
        .expect("restart should leave the whole dynamic world dormant");
    assert_eq!(runtime.dormant_backing_keys().len(), 4);

    runtime
        .run_action("purgeRoot")
        .expect("purge should clean foreign occurrences while terminating the provenance subtree");

    let surviving_keys = runtime.dormant_backing_keys();
    assert_eq!(
        surviving_keys.len(),
        1,
        "only the foreign owner should remain a live dormant identity"
    );
    let foreign_key = surviving_keys[0].clone();

    let mut provider = runtime.into_provider();
    assert_eq!(
        provider.loads,
        vec![foreign_key.clone()],
        "purge should privately read only the surviving dormant owner selected by cleanup summary"
    );
    assert_eq!(provider.replacements.len(), 1);
    assert_eq!(
        provider.replacements[0],
        vec![foreign_key.clone()],
        "purge publication should rewrite only the surviving foreign backing"
    );
    assert_eq!(
        provider.backing.len(),
        4,
        "terminated opaque backing may remain physically present without remaining semantically live"
    );

    provider.loads.clear();
    provider.replacements.clear();
    let mut restarted = PartialPersistentRuntime::open(checked, provider)
        .expect("accepted purge world should restart");
    assert_eq!(restarted.dormant_backing_keys(), vec![foreign_key.clone()]);
    restarted
        .run_action("proveRightOwnsForeign")
        .expect("surviving foreign owner should retain right-root provenance");

    let provider = restarted.into_provider();
    assert!(
        provider.loads.is_empty(),
        "metadata-only provenance proof should keep the foreign owner dormant"
    );

    let mut restarted = PartialPersistentRuntime::open(
        check_source_with_runtime_models(SOURCE).expect("source should still check"),
        provider,
    )
    .expect("metadata proof should leave the world restartable");
    restarted
        .materialize_designation("foreign")
        .expect("surviving foreign owner should remain explicitly materializable");
    let error = restarted.run_action("selectForeignChild").expect_err(
        "purged child and grandchild occurrences must be absent from foreign membership",
    );
    assert!(
        error.message.contains("out of bounds"),
        "foreign membership should be empty after purge cleanup: {}",
        error.message
    );
    restarted
        .run_action("readForeignName")
        .expect("foreign survivor scalar payload should remain intact");
    assert_eq!(
        restarted.value("observedName").unwrap(),
        Value::String("Foreign survivor".into())
    );

    let provider = restarted.into_provider();
    assert_eq!(
        provider.loads,
        vec![foreign_key],
        "explicit later materialization should read the same rewritten foreign backing exactly once"
    );
}
