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

    create Node in foreign as childNode {
        through childNode.name = "Child"
        insert childNode into foreign.children
    }
    child = foreign.children[0]

    create Node in child as grandchildNode {
        through grandchildNode.name = "Grandchild"
        insert grandchildNode into child.children
    }
    grandchild = child.children[0]
}

action transferInThenPurge {
    transfer child from foreign to root
    purge root in left
}

action proveRightOwnsForeign {
    transfer foreign from right to right
}

action proveLeftOwnsRoot {
    transfer root from left to left
}

action proveForeignOwnsChild {
    transfer child from foreign to foreign
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
fn staged_transfer_in_adds_dormant_branch_to_purge_set_and_cleans_surviving_owner() {
    let checked = check_source_with_runtime_models(SOURCE)
        .expect("dormant staged transfer-in purge pressure source should check");
    let mut initial = PartialPersistentRuntime::open(checked.clone(), MemoryProvider::default())
        .expect("fresh partial runtime should open");
    initial.run_action("seed").expect("seed should publish");

    let mut provider = initial.into_provider();
    provider.loads.clear();
    provider.replacements.clear();
    assert_eq!(
        provider.backing.len(),
        4,
        "seed should produce root, foreign, child, and grandchild backing"
    );

    let mut runtime = PartialPersistentRuntime::open(checked.clone(), provider)
        .expect("restart should leave the whole dynamic world dormant");
    assert_eq!(runtime.dormant_backing_keys().len(), 4);

    runtime
        .run_action("transferInThenPurge")
        .expect("staged transfer in must add child branch to the purge set");

    let surviving_keys = runtime.dormant_backing_keys();
    assert_eq!(
        surviving_keys.len(),
        1,
        "root, child, and grandchild should terminate while foreign survives dormant"
    );
    let foreign_key = surviving_keys[0].clone();

    let mut provider = runtime.into_provider();
    assert_eq!(
        provider.loads,
        vec![foreign_key.clone()],
        "purge should privately read only the surviving foreign owner whose membership contains child"
    );
    assert_eq!(provider.replacements.len(), 1);
    assert_eq!(
        provider.replacements[0],
        vec![foreign_key.clone()],
        "publication should rewrite only the surviving foreign backing"
    );
    assert_eq!(
        provider.backing.len(),
        4,
        "terminated opaque backing may remain physically present"
    );

    provider.loads.clear();
    provider.replacements.clear();
    let mut restarted = PartialPersistentRuntime::open(checked, provider)
        .expect("accepted transfer-in purge world should restart");
    restarted
        .run_action("proveRightOwnsForeign")
        .expect("foreign survivor should retain right-root provenance");
    restarted
        .run_action("proveLeftOwnsRoot")
        .expect_err("purged root should no longer be live");
    restarted
        .run_action("proveForeignOwnsChild")
        .expect_err("purged child should no longer be live beneath foreign");

    let provider = restarted.into_provider();
    assert!(
        provider.loads.is_empty(),
        "post-purge provenance proofs should remain metadata-only"
    );

    let mut restarted = PartialPersistentRuntime::open(
        check_source_with_runtime_models(SOURCE).expect("source should still check"),
        provider,
    )
    .expect("metadata proofs should leave the world restartable");
    restarted
        .materialize_designation("foreign")
        .expect("foreign survivor should remain explicitly materializable");
    let error = restarted
        .run_action("selectForeignChild")
        .expect_err("purged child occurrence must be absent from foreign membership");
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
        "later explicit materialization should read exactly the rewritten foreign backing"
    );
}
