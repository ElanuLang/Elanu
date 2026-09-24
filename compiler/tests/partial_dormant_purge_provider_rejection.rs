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

action proveLeftOwnsRoot {
    transfer root from left to left
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
    attempts: Vec<Vec<Vec<u8>>>,
    replacements: Vec<Vec<Vec<u8>>>,
    reject_next: bool,
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
        let replacement_keys = backing_replacements
            .iter()
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        self.attempts.push(replacement_keys.clone());

        if self.reject_next {
            self.reject_next = false;
            return Err(RuntimeError::new("provider rejected candidate"));
        }

        let mut candidate = self.backing.clone();
        for (key, payload) in backing_replacements {
            candidate.insert(key.clone(), payload.clone());
        }
        self.backing = candidate;
        self.manifest = Some(manifest.to_vec());
        self.replacements.push(replacement_keys);
        Ok(())
    }
}

#[test]
fn rejected_dormant_purge_preserves_foreign_backing_and_retries_atomically() {
    let checked = check_source_with_runtime_models(SOURCE)
        .expect("dormant purge rejection pressure source should check");
    let mut initial = PartialPersistentRuntime::open(checked.clone(), MemoryProvider::default())
        .expect("fresh partial runtime should open");
    initial.run_action("seed").expect("seed should publish");

    let mut provider = initial.into_provider();
    provider.loads.clear();
    provider.attempts.clear();
    provider.replacements.clear();
    let manifest_before = provider.manifest.clone();
    let backing_before = provider.backing.clone();
    assert_eq!(backing_before.len(), 4);
    provider.reject_next = true;

    let mut runtime = PartialPersistentRuntime::open(checked.clone(), provider)
        .expect("restart should leave the whole dynamic world dormant");
    assert_eq!(runtime.dormant_backing_keys().len(), 4);

    let error = runtime
        .run_action("purgeRoot")
        .expect_err("provider rejection must reject purge publication atomically");
    assert!(
        error.message.contains("provider rejected candidate"),
        "failure should come from provider rejection: {}",
        error.message
    );
    assert_eq!(
        runtime.dormant_backing_keys().len(),
        4,
        "rejected purge must leave every prior modeled identity live and dormant"
    );
    runtime
        .run_action("proveLeftOwnsRoot")
        .expect("rejected purge must preserve root provenance in the authoritative runtime");
    runtime
        .run_action("proveRightOwnsForeign")
        .expect("rejected purge must preserve foreign provenance in the authoritative runtime");

    let mut rejected = runtime.into_provider();
    assert_eq!(rejected.manifest, manifest_before);
    assert_eq!(rejected.backing, backing_before);
    assert_eq!(
        rejected.loads.len(),
        1,
        "rejected purge may privately read only the surviving dormant foreign owner"
    );
    let foreign_key = rejected.loads[0].clone();
    assert_eq!(
        rejected.attempts,
        vec![vec![foreign_key.clone()]],
        "rejected candidate should contain exactly the foreign backing cleanup"
    );
    assert!(
        rejected.replacements.is_empty(),
        "rejected candidate must not become an accepted backing replacement"
    );

    let mut inspection_provider = rejected.clone();
    inspection_provider.loads.clear();
    inspection_provider.attempts.clear();
    inspection_provider.replacements.clear();
    let mut inspection = PartialPersistentRuntime::open(checked.clone(), inspection_provider)
        .expect("rejected durable world should remain restartable");
    inspection
        .materialize_designation("foreign")
        .expect("foreign owner should materialize from the unchanged pre-purge backing");
    inspection
        .run_action("selectForeignChild")
        .expect("rejected cleanup must not remove the original foreign membership");
    let inspection_provider = inspection.into_provider();
    assert_eq!(
        inspection_provider.loads,
        vec![foreign_key.clone()],
        "inspection should read only the unchanged foreign backing"
    );

    rejected.loads.clear();
    rejected.attempts.clear();
    rejected.replacements.clear();
    let mut retried = PartialPersistentRuntime::open(checked.clone(), rejected)
        .expect("rejected durable world should remain retryable");
    retried
        .run_action("purgeRoot")
        .expect("the identical purge should succeed once the provider accepts it");
    assert_eq!(
        retried.dormant_backing_keys(),
        vec![foreign_key.clone()],
        "accepted retry should leave only the foreign survivor live and dormant"
    );

    let mut accepted = retried.into_provider();
    assert_eq!(
        accepted.loads,
        vec![foreign_key.clone()],
        "retry should again read only the surviving foreign backing"
    );
    assert_eq!(accepted.attempts, vec![vec![foreign_key.clone()]]);
    assert_eq!(accepted.replacements, vec![vec![foreign_key.clone()]]);
    assert_eq!(
        accepted.backing.len(),
        4,
        "physical backing for terminated identities may remain after accepted purge"
    );

    accepted.loads.clear();
    accepted.attempts.clear();
    accepted.replacements.clear();
    let mut restarted = PartialPersistentRuntime::open(checked, accepted)
        .expect("accepted retry world should restart");
    restarted
        .run_action("proveRightOwnsForeign")
        .expect("foreign survivor should retain right-root provenance after retry");
    restarted
        .run_action("proveLeftOwnsRoot")
        .expect_err("accepted retry must retire the purged root identity");
    restarted
        .materialize_designation("foreign")
        .expect("foreign survivor should remain explicitly materializable");
    let error = restarted.run_action("selectForeignChild").expect_err(
        "accepted retry must publish the cleaned foreign membership",
    );
    assert!(
        error.message.contains("out of bounds"),
        "foreign membership should be empty after accepted retry: {}",
        error.message
    );
    restarted
        .run_action("readForeignName")
        .expect("foreign scalar payload should remain intact after accepted retry");
    assert_eq!(
        restarted.value("observedName").unwrap(),
        Value::String("Foreign survivor".into())
    );

    let accepted = restarted.into_provider();
    assert_eq!(
        accepted.loads,
        vec![foreign_key],
        "later explicit materialization should read exactly the rewritten foreign backing"
    );
}
