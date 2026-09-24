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
state sourceFolder: maybe live Folder = none
state selectedFolder: maybe live Folder = none
state restoreDestination: maybe live Folder = none
state observedDestinationName = ""

action seed {
    create Folder in workspace as source {
        through source.name = "Source"
        insert source into workspace.folders
    }
    create Folder in workspace as selected {
        through selected.name = "Selected"
        insert selected into workspace.folders
    }
    sourceFolder = workspace.folders[0]
    selectedFolder = workspace.folders[1]
}

action rememberParent {
    through selectedFolder.restoreParent = sourceFolder
}

action copyRememberedParent {
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
fn model_local_designation_copy_preserves_identity_without_loading_target_backing() {
    let checked = check_source_with_runtime_models(SOURCE)
        .expect("model-local designation copy pressure source should check");
    let mut initial = PartialPersistentRuntime::open(checked.clone(), MemoryProvider::default())
        .expect("fresh partial runtime should open");
    initial.run_action("seed").expect("seed should publish");
    initial
        .run_action("rememberParent")
        .expect("model-local remembered parent should publish");

    let mut provider = initial.into_provider();
    provider.loads.clear();
    provider.replacements.clear();
    assert_eq!(
        provider.backing.len(),
        2,
        "seed should produce two folder backings"
    );

    let mut runtime = PartialPersistentRuntime::open(checked.clone(), provider)
        .expect("restart should leave both dynamic folders dormant");
    assert_eq!(runtime.dormant_backing_keys().len(), 2);

    runtime
        .materialize_designation("selectedFolder")
        .expect("host should materialize only the folder owning restoreParent");
    assert_eq!(
        runtime.dormant_backing_keys().len(),
        1,
        "remembered target should remain dormant"
    );

    runtime
        .run_action("copyRememberedParent")
        .expect("source should copy the stored model-local designation identity");

    let mut provider = runtime.into_provider();
    assert_eq!(
        provider.loads.len(),
        1,
        "copying a stored designation must not materialize its target"
    );
    let selected_key = provider.loads[0].clone();
    assert_eq!(
        provider.replacements.len(),
        1,
        "successful top-level designation copy should publish one candidate"
    );
    assert!(
        provider.replacements[0].is_empty(),
        "copying only top-level designation state must not rewrite dynamic backing"
    );

    provider.loads.clear();
    provider.replacements.clear();
    let mut restarted = PartialPersistentRuntime::open(checked, provider)
        .expect("copied designation should survive restart while both folders are dormant");
    assert_eq!(restarted.dormant_backing_keys().len(), 2);

    restarted
        .materialize_designation("restoreDestination")
        .expect("later explicit materialization should load the remembered target");
    assert_eq!(
        restarted.dormant_backing_keys().len(),
        1,
        "only the copied designation target should become resident"
    );
    restarted
        .run_action("observeDestination")
        .expect("remembered target payload should be readable after explicit materialization");
    assert_eq!(
        restarted.value("observedDestinationName").unwrap(),
        Value::String("Source".into())
    );

    let provider = restarted.into_provider();
    assert_eq!(provider.loads.len(), 1);
    assert_ne!(
        provider.loads[0], selected_key,
        "later materialization must load the distinct remembered target backing"
    );
}
