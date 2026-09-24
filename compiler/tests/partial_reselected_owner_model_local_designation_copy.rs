use std::collections::HashMap;

use elanu_compiler::{
    check_source_with_runtime_models,
    runtime::{
        persistence::partial::{PartialPersistenceProvider, PartialPersistentRuntime},
        RuntimeError, Value,
    },
};

const SOURCE: &str = r#"
state model Folder {
    state name = ""
    state folders: [live Folder] = []
    state restoreParent: maybe live Folder = none
}

state workspace: Folder
state firstOwner: maybe live Folder = none
state secondOwner: maybe live Folder = none
state firstTarget: maybe live Folder = none
state secondTarget: maybe live Folder = none
state selectedFolder: maybe live Folder = none
state restoreDestination: maybe live Folder = none
state observedDestinationName = ""

action seed {
    create Folder in workspace as ownerA {
        through ownerA.name = "OwnerA"
        insert ownerA into workspace.folders
    }
    create Folder in workspace as ownerB {
        through ownerB.name = "OwnerB"
        insert ownerB into workspace.folders
    }
    create Folder in workspace as targetA {
        through targetA.name = "TargetA"
        insert targetA into workspace.folders
    }
    create Folder in workspace as targetB {
        through targetB.name = "TargetB"
        insert targetB into workspace.folders
    }

    firstOwner = workspace.folders[0]
    secondOwner = workspace.folders[1]
    firstTarget = workspace.folders[2]
    secondTarget = workspace.folders[3]
    selectedFolder = workspace.folders[0]
}

action rememberTargets {
    through firstOwner.restoreParent = firstTarget
    through secondOwner.restoreParent = secondTarget
}

action selectSecondOwnerAndCopy {
    selectedFolder = workspace.folders[1]
    restoreDestination = selectedFolder.restoreParent
}

action observeDestination {
    observedDestinationName = restoreDestination.name
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
fn reselected_owner_drives_model_local_designation_copy_without_target_materialization() {
    let checked = check_source_with_runtime_models(SOURCE)
        .expect("reselected-owner model-local designation copy pressure source should check");
    let mut initial = PartialPersistentRuntime::open(checked.clone(), MemoryProvider::default())
        .expect("fresh partial runtime should open");
    initial.run_action("seed").expect("seed should publish");
    initial
        .run_action("rememberTargets")
        .expect("both model-local remembered targets should publish");

    let mut provider = initial.into_provider();
    provider.loads.clear();
    provider.replacements.clear();
    assert_eq!(
        provider.backing.len(),
        4,
        "seed should produce four folder backings"
    );

    let mut runtime = PartialPersistentRuntime::open(checked.clone(), provider)
        .expect("restart should leave all four dynamic folders dormant");
    assert_eq!(runtime.dormant_backing_keys().len(), 4);

    runtime
        .materialize_designation("firstOwner")
        .expect("host should materialize owner A");
    runtime
        .materialize_designation("secondOwner")
        .expect("host should materialize owner B");
    assert_eq!(
        runtime.dormant_backing_keys().len(),
        2,
        "both remembered targets should remain dormant"
    );

    runtime
        .run_action("selectSecondOwnerAndCopy")
        .expect("same transaction owner reselection should drive model-local designation copy");

    let mut provider = runtime.into_provider();
    assert_eq!(
        provider.loads.len(),
        2,
        "source owner reselection and designation copy must not materialize either remembered target"
    );
    let first_owner_key = provider.loads[0].clone();
    let second_owner_key = provider.loads[1].clone();
    assert_ne!(
        first_owner_key, second_owner_key,
        "the two owners must have distinct backing identities"
    );
    assert_eq!(
        provider.replacements.len(),
        1,
        "successful top-level reselection/copy should publish one candidate"
    );
    assert!(
        provider.replacements[0].is_empty(),
        "owner reselection and designation transport alone must not rewrite dynamic backing"
    );

    provider.loads.clear();
    provider.replacements.clear();
    let mut restarted = PartialPersistentRuntime::open(checked, provider)
        .expect("reselected owner and copied target should survive restart");
    assert_eq!(restarted.dormant_backing_keys().len(), 4);

    restarted
        .materialize_designation("restoreDestination")
        .expect("later explicit materialization should load owner B's remembered target");
    assert_eq!(
        restarted.dormant_backing_keys().len(),
        3,
        "only the copied designation target should become resident"
    );
    restarted
        .run_action("observeDestination")
        .expect("owner B's remembered target payload should be readable after materialization");
    assert_eq!(
        restarted.value("observedDestinationName").unwrap(),
        Value::String("TargetB".into())
    );

    let provider = restarted.into_provider();
    assert_eq!(provider.loads.len(), 1);
    assert_ne!(provider.loads[0], first_owner_key);
    assert_ne!(provider.loads[0], second_owner_key);
}
