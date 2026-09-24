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
state observedChildName = ""
state observedGrandchildName = ""

action seed {
    create Node in left as rootNode {
        through rootNode.name = "Root"
        insert rootNode into left.children
    }
    root = left.children[0]

    create Node in right as foreignNode {
        through foreignNode.name = "Foreign"
        insert foreignNode into right.children
    }
    foreign = right.children[0]

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
}

action transferOutThenPurge {
    transfer child from root to foreign
    purge root in left
}

action proveForeignOwnsChild {
    transfer child from foreign to foreign
}

action proveChildOwnsGrandchild {
    transfer grandchild from child to child
}

action proveLeftOwnsRoot {
    transfer root from left to left
}

action readChildName {
    observedChildName = child.name
}

action readGrandchildName {
    observedGrandchildName = grandchild.name
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
fn staged_transfer_out_removes_dormant_branch_from_purge_set_without_payload_reads() {
    let checked = check_source_with_runtime_models(SOURCE)
        .expect("dormant staged transfer-out purge pressure source should check");
    let mut initial = PartialPersistentRuntime::open(checked.clone(), MemoryProvider::default())
        .expect("fresh partial runtime should open");
    initial.run_action("seed").expect("seed should publish");

    let mut provider = initial.into_provider();
    provider.loads.clear();
    provider.replacements.clear();
    assert_eq!(
        provider.backing.len(),
        4,
        "seed should produce root, child, grandchild, and foreign backing"
    );

    let mut runtime = PartialPersistentRuntime::open(checked.clone(), provider)
        .expect("restart should leave the whole dynamic world dormant");
    assert_eq!(runtime.dormant_backing_keys().len(), 4);

    runtime
        .run_action("transferOutThenPurge")
        .expect("staged transfer out must remove child branch from the purge set");
    assert_eq!(
        runtime.dormant_backing_keys().len(),
        3,
        "only the purged root should cease to be a live dormant identity"
    );

    let mut provider = runtime.into_provider();
    assert!(
        provider.loads.is_empty(),
        "purge-set selection and root cleanup should require no dynamic backing reads"
    );
    assert_eq!(provider.replacements, vec![Vec::<Vec<u8>>::new()]);
    assert_eq!(
        provider.backing.len(),
        4,
        "terminated root bytes may remain physically present without remaining semantically live"
    );

    provider.loads.clear();
    provider.replacements.clear();
    let mut restarted = PartialPersistentRuntime::open(checked, provider)
        .expect("accepted transfer-and-purge world should restart");
    restarted
        .run_action("proveForeignOwnsChild")
        .expect("staged child transfer must publish foreign as the child's owner");
    restarted
        .run_action("proveChildOwnsGrandchild")
        .expect("grandchild provenance must remain beneath the transferred child");
    restarted
        .run_action("proveLeftOwnsRoot")
        .expect_err("purged root must no longer be a live modeled identity");

    let provider = restarted.into_provider();
    assert!(
        provider.loads.is_empty(),
        "post-restart provenance proofs should remain metadata-only"
    );

    let mut restarted = PartialPersistentRuntime::open(
        check_source_with_runtime_models(SOURCE).expect("source should still check"),
        provider,
    )
    .expect("metadata proofs should leave the world restartable");
    restarted
        .materialize_designation("child")
        .expect("transferred child should remain explicitly materializable");
    restarted
        .run_action("readChildName")
        .expect("transferred child payload should remain intact");
    assert_eq!(
        restarted.value("observedChildName").unwrap(),
        Value::String("Child".into())
    );

    restarted
        .materialize_designation("grandchild")
        .expect("surviving grandchild should remain explicitly materializable");
    restarted
        .run_action("readGrandchildName")
        .expect("surviving grandchild payload should remain intact");
    assert_eq!(
        restarted.value("observedGrandchildName").unwrap(),
        Value::String("Grandchild".into())
    );

    let provider = restarted.into_provider();
    assert_eq!(
        provider.loads.len(),
        2,
        "only later explicit child and grandchild materialization should read dynamic backing"
    );
    assert_ne!(
        provider.loads[0], provider.loads[1],
        "child and grandchild should retain distinct backing identities"
    );
}
