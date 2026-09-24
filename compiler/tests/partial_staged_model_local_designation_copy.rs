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
state firstSource: maybe live Folder = none
state secondSource: maybe live Folder = none
state selectedFolder: maybe live Folder = none
state restoreDestination: maybe live Folder = none
state observedDestinationName = ""

action seed {
    create Folder in workspace as first {
        through first.name = "FirstSource"
        insert first into workspace.folders
    }
    create Folder in workspace as second {
        through second.name = "SecondSource"
        insert second into workspace.folders
    }
    create Folder in workspace as selected {
        through selected.name = "Selected"
        insert selected into workspace.folders
    }
    firstSource = workspace.folders[0]
    secondSource = workspace.folders[1]
    selectedFolder = workspace.folders[2]
}

action rememberFirst {
    through selectedFolder.restoreParent = firstSource
}

action retargetAndCopy {
    through selectedFolder.restoreParent = secondSource
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
fn staged_model_local_designation_retarget_is_visible_to_same_transaction_copy() {
    let checked = check_source_with_runtime_models(SOURCE)
        .expect("staged model-local designation copy pressure source should check");
    let mut initial = PartialPersistentRuntime::open(checked.clone(), MemoryProvider::default())
        .expect("fresh partial runtime should open");
    initial.run_action("seed").expect("seed should publish");
    initial
        .run_action("rememberFirst")
        .expect("initial model-local designation should publish");

    let mut provider = initial.into_provider();
    provider.loads.clear();
    provider.replacements.clear();
    assert_eq!(
        provider.backing.len(),
        3,
        "seed should produce three folder backings"
    );

    let mut runtime = PartialPersistentRuntime::open(checked.clone(), provider)
        .expect("restart should leave all dynamic folders dormant");
    assert_eq!(runtime.dormant_backing_keys().len(), 3);

    runtime
        .materialize_designation("selectedFolder")
        .expect("host should materialize only the folder owning restoreParent");
    assert_eq!(
        runtime.dormant_backing_keys().len(),
        2,
        "both possible designation targets should remain dormant"
    );

    runtime
        .run_action("retargetAndCopy")
        .expect("same transaction copy should observe staged model-local designation target");

    let mut provider = runtime.into_provider();
    assert_eq!(
        provider.loads.len(),
        1,
        "retargeting and copying stored identity must not materialize either target"
    );
    let selected_key = provider.loads[0].clone();
    assert_eq!(
        provider.replacements,
        vec![vec![selected_key.clone()]],
        "only the materialized owner backing changed when its model-local slot was retargeted"
    );

    provider.loads.clear();
    provider.replacements.clear();
    let mut restarted = PartialPersistentRuntime::open(checked, provider)
        .expect("staged retarget and copied designation should survive restart");
    assert_eq!(restarted.dormant_backing_keys().len(), 3);

    restarted
        .materialize_designation("restoreDestination")
        .expect("later explicit materialization should load the newly remembered target");
    assert_eq!(
        restarted.dormant_backing_keys().len(),
        2,
        "only the copied designation target should become resident"
    );
    restarted
        .run_action("observeDestination")
        .expect("newly remembered target payload should be readable after materialization");
    assert_eq!(
        restarted.value("observedDestinationName").unwrap(),
        Value::String("SecondSource".into())
    );

    let provider = restarted.into_provider();
    assert_eq!(provider.loads.len(), 1);
    assert_ne!(
        provider.loads[0], selected_key,
        "later target materialization must load a distinct dynamic backing"
    );
}
