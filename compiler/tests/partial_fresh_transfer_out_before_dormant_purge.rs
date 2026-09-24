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
state root: maybe live Node = none
state fresh: maybe live Node = none
state observedName = ""

action seed {
    create Node in left as rootNode {
        through rootNode.name = "Committed root"
        root = rootNode
    }
}

action createTransferOutThenPurge {
    create Node in root as freshNode {
        through freshNode.name = "Fresh candidate"
        fresh = freshNode
    }
    transfer fresh from root to right
    purge root in left
}

action proveLeftOwnsRoot {
    transfer root from left to left
}

action readRootName {
    observedName = root.name
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
fn persisted_fresh_designation_does_not_bypass_committed_only_transfer_law() {
    let checked = check_source_with_runtime_models(SOURCE)
        .expect("fresh designation plus transfer source should remain compiler-accepted");
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
        1,
        "seed should produce only committed-root backing"
    );

    let mut runtime = PartialPersistentRuntime::open(checked.clone(), provider)
        .expect("restart should leave the committed root dormant");
    assert_eq!(runtime.dormant_backing_keys().len(), 1);

    let error = runtime
        .run_action("createTransferOutThenPurge")
        .expect_err("fresh transaction-local children are not transfer targets");
    assert!(
        error
            .message
            .contains("requires an existing committed dynamic child"),
        "failure should preserve the committed-only transfer law: {}",
        error.message
    );
    assert_eq!(
        runtime.dormant_backing_keys().len(),
        1,
        "failed transfer must roll back the fresh identity completely"
    );

    let provider = runtime.into_provider();
    assert_eq!(provider.manifest, manifest_before);
    assert_eq!(provider.backing, backing_before);
    assert!(
        provider.loads.is_empty(),
        "committed-only transfer validation should not materialize the dormant source owner"
    );
    assert!(
        provider.replacements.is_empty(),
        "failed fresh transfer must not attempt durable publication"
    );

    let mut proof_provider = provider.clone();
    proof_provider.loads.clear();
    proof_provider.replacements.clear();
    let mut proof = PartialPersistentRuntime::open(checked.clone(), proof_provider)
        .expect("failed fresh transfer must leave the prior durable world restartable");
    proof
        .run_action("proveLeftOwnsRoot")
        .expect("committed root provenance must survive the failed action");
    proof
        .materialize_designation("fresh")
        .expect_err("fresh designation assignment must roll back to absence");
    assert!(
        proof.into_provider().loads.is_empty(),
        "metadata proof and absent fresh designation should require no backing read"
    );

    let mut provider = provider;
    provider.loads.clear();
    provider.replacements.clear();
    let mut restarted = PartialPersistentRuntime::open(checked, provider)
        .expect("prior committed root world should remain restartable");
    restarted
        .materialize_designation("root")
        .expect("committed root should remain explicitly materializable");
    restarted
        .run_action("readRootName")
        .expect("failed fresh transfer must preserve committed root payload");
    assert_eq!(
        restarted.value("observedName").unwrap(),
        Value::String("Committed root".into())
    );

    let provider = restarted.into_provider();
    assert_eq!(
        provider.loads.len(),
        1,
        "only later explicit root materialization should read backing"
    );
}
