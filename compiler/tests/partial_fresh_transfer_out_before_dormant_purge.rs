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
        through freshNode.name = "Fresh survivor"
        fresh = freshNode
    }
    transfer fresh from root to right
    purge root in left
}

action proveRightOwnsFresh {
    transfer fresh from right to right
}

action proveRootStillLive {
    transfer root from left to left
}

action readFreshName {
    observedName = fresh.name
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
fn fresh_transfer_out_removes_dormant_purge_blocker_and_publishes_survivor() {
    let checked = check_source_with_runtime_models(SOURCE)
        .expect("fresh transfer-out dormant purge pressure source should check");
    let mut initial = PartialPersistentRuntime::open(checked.clone(), MemoryProvider::default())
        .expect("fresh partial runtime should open");
    initial.run_action("seed").expect("seed should publish");

    let mut provider = initial.into_provider();
    provider.loads.clear();
    provider.replacements.clear();
    assert_eq!(
        provider.backing.len(),
        1,
        "seed should produce only committed-root backing"
    );

    let mut runtime = PartialPersistentRuntime::open(checked.clone(), provider)
        .expect("restart should leave the committed root dormant");
    let root_keys = runtime.dormant_backing_keys();
    assert_eq!(root_keys.len(), 1);
    let root_key = root_keys[0].clone();

    runtime
        .run_action("createTransferOutThenPurge")
        .expect("fresh child transferred out before purge should survive and commit");

    let mut provider = runtime.into_provider();
    assert!(
        provider.loads.is_empty(),
        "create, transfer, and purge should not materialize the dormant committed root"
    );
    assert_eq!(
        provider.replacements.len(),
        1,
        "accepted action should publish one durable candidate"
    );
    assert_eq!(
        provider.replacements[0].len(),
        1,
        "only the fresh surviving identity should need new backing publication"
    );
    let fresh_key = provider.replacements[0][0].clone();
    assert_ne!(
        fresh_key, root_key,
        "fresh survivor must receive its own backing rather than overwrite the retired root"
    );
    assert_eq!(
        provider.backing.len(),
        2,
        "retired root bytes may remain physically present beside fresh survivor backing"
    );

    provider.loads.clear();
    provider.replacements.clear();
    let mut restarted = PartialPersistentRuntime::open(checked, provider)
        .expect("accepted fresh-transfer-and-purge world should restart");
    assert_eq!(
        restarted.dormant_backing_keys(),
        vec![fresh_key.clone()],
        "only the fresh survivor should remain a live dormant modeled identity"
    );
    restarted
        .run_action("proveRightOwnsFresh")
        .expect("fresh survivor should commit under its transferred right owner");
    restarted
        .run_action("proveRootStillLive")
        .expect_err("purged root must no longer provide a live target after restart");

    let mut provider = restarted.into_provider();
    assert!(
        provider.loads.is_empty(),
        "post-restart provenance proofs should remain metadata-only"
    );
    provider.loads.clear();
    provider.replacements.clear();

    let mut restarted = PartialPersistentRuntime::open(
        check_source_with_runtime_models(SOURCE).expect("source should still check"),
        provider,
    )
    .expect("fresh survivor world should remain restartable");
    restarted
        .materialize_designation("fresh")
        .expect("fresh survivor should be explicitly materializable after restart");
    restarted
        .run_action("readFreshName")
        .expect("fresh survivor payload should remain intact");
    assert_eq!(
        restarted.value("observedName").unwrap(),
        Value::String("Fresh survivor".into())
    );

    let provider = restarted.into_provider();
    assert_eq!(
        provider.loads,
        vec![fresh_key],
        "only explicit later fresh-survivor materialization should read backing"
    );
}
