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
    state pinnedNode: maybe live Node = none
}

state left: Node
state right: Node
state root: maybe live Node = none
state child: maybe live Node = none
state grandchild: maybe live Node = none
state foreign: maybe live Node = none
state observedPinned: maybe live Node = none
state observedName = ""

action seed {
    create Node in left as rootNode {
        through rootNode.name = "Root"
        insert rootNode into left.children
        root = rootNode
    }

    create Node in root as childNode {
        through childNode.name = "Child"
        insert childNode into root.children
        child = childNode
    }

    create Node in child as grandchildNode {
        through grandchildNode.name = "Grandchild"
        insert grandchildNode into child.children
        grandchild = grandchildNode
    }

    create Node in right as foreignNode {
        through foreignNode.name = "Foreign survivor"
        insert foreignNode into right.children
        foreign = foreignNode
    }

    through foreign.pinnedNode = grandchild
}

action purgeRoot {
    purge root in left
}

action proveRightOwnsForeign {
    transfer foreign from right to right
}

action inspectForeign {
    observedPinned = foreign.pinnedNode
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
fn purge_clears_model_local_optional_designation_in_dormant_survivor_without_loading_doomed_payloads() {
    let checked = check_source_with_runtime_models(SOURCE)
        .expect("dormant purge model-local optional cleanup source should check");
    let mut initial = PartialPersistentRuntime::open(checked.clone(), MemoryProvider::default())
        .expect("fresh partial runtime should open");
    initial.run_action("seed").expect("seed should publish");

    let mut provider = initial.into_provider();
    provider.loads.clear();
    provider.replacements.clear();
    assert_eq!(
        provider.backing.len(),
        4,
        "seed should produce R, C, G, and F backing"
    );

    let mut runtime = PartialPersistentRuntime::open(checked.clone(), provider)
        .expect("restart should leave all dynamic identities dormant");
    assert_eq!(runtime.dormant_backing_keys().len(), 4);

    runtime
        .run_action("purgeRoot")
        .expect("purge should reuse dormant optional-designation cleanup");

    let dormant = runtime.dormant_backing_keys();
    assert_eq!(
        dormant.len(),
        1,
        "only foreign survivor F should remain live and dormant"
    );
    let foreign_key = dormant[0].clone();

    let provider = runtime.into_provider();
    assert_eq!(
        provider.loads,
        vec![foreign_key.clone()],
        "purge should privately read only the affected surviving foreign backing"
    );
    assert_eq!(
        provider.replacements.last(),
        Some(&vec![foreign_key.clone()]),
        "purge should rewrite exactly the surviving foreign backing"
    );

    let mut proof_provider = provider.clone();
    proof_provider.loads.clear();
    proof_provider.replacements.clear();
    let mut proof = PartialPersistentRuntime::open(checked.clone(), proof_provider)
        .expect("post-purge world should restart with F dormant");
    proof
        .run_action("proveRightOwnsForeign")
        .expect("F provenance should remain metadata-provable");
    assert!(
        proof.into_provider().loads.is_empty(),
        "provenance proof should not materialize F"
    );

    let mut provider = provider;
    provider.loads.clear();
    provider.replacements.clear();
    let mut restarted = PartialPersistentRuntime::open(checked, provider)
        .expect("cleaned post-purge world should restart");
    restarted
        .materialize_designation("foreign")
        .expect("foreign survivor should remain independently materializable");
    restarted
        .run_action("inspectForeign")
        .expect("foreign scalar payload and cleared optional designation should remain readable");
    assert_eq!(
        restarted.value("observedName").unwrap(),
        Value::String("Foreign survivor".into())
    );
    let error = restarted
        .materialize_designation("observedPinned")
        .expect_err("purged grandchild must leave the copied model-local optional designation absent");
    assert!(
        error.message.contains("has no target"),
        "model-local optional designation must not retain a purged identity: {}",
        error.message
    );

    let provider = restarted.into_provider();
    assert_eq!(
        provider.loads,
        vec![foreign_key],
        "post-purge observation should materialize only F"
    );
}
