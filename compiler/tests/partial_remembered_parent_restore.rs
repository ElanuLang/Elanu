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
state trashFolder: maybe live Folder = none
state selectedFolder: maybe live Folder = none
state restoreDestination: maybe live Folder = none

derived inSource = selectedFolder is in sourceFolder.folders
derived inTrash = selectedFolder is in trashFolder.folders
derived restoreDestinationPresent = restoreDestination is present

action seed {
    create Folder in workspace as source {
        through source.name = "Source"
        insert source into workspace.folders
    }
    create Folder in workspace as trash {
        through trash.name = "Trash"
        insert trash into workspace.folders
    }

    sourceFolder = workspace.folders[0]
    trashFolder = workspace.folders[1]

    create Folder in sourceFolder as selected {
        through selected.name = "Selected"
        insert selected into sourceFolder.folders
    }
    selectedFolder = sourceFolder.folders[0]
    through selectedFolder.restoreParent = sourceFolder
}

action moveToTrash {
    transfer selectedFolder from sourceFolder to trashFolder
    insert selectedFolder into trashFolder.folders
    remove selectedFolder from sourceFolder.folders
}

action restoreFromTrash {
    restoreDestination = selectedFolder.restoreParent
    transfer selectedFolder from trashFolder to restoreDestination
    insert selectedFolder into restoreDestination.folders
    remove selectedFolder from trashFolder.folders
    through selectedFolder.restoreParent = none
}

action inspectRememberedParent {
    restoreDestination = selectedFolder.restoreParent
}

action proveRestoredOwner {
    transfer selectedFolder from sourceFolder to sourceFolder
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
fn remembered_parent_identity_feeds_partial_restore_without_reconstruction() {
    let checked = check_source_with_runtime_models(SOURCE)
        .expect("partial remembered-parent restore pressure source should check");
    let mut initial = PartialPersistentRuntime::open(checked.clone(), MemoryProvider::default())
        .expect("fresh partial runtime should open");
    initial.run_action("seed").expect("seed should publish");
    initial
        .run_action("moveToTrash")
        .expect("move to trash should publish before restart");

    let mut provider = initial.into_provider();
    provider.loads.clear();
    provider.replacements.clear();
    assert_eq!(
        provider.backing.len(),
        3,
        "seed should produce source, trash, and selected folder backings"
    );

    let mut runtime = PartialPersistentRuntime::open(checked.clone(), provider)
        .expect("restart should leave all dynamic folders dormant");
    assert_eq!(runtime.dormant_backing_keys().len(), 3);

    runtime
        .materialize_designation("sourceFolder")
        .expect("host should materialize the remembered destination owner");
    runtime
        .materialize_designation("trashFolder")
        .expect("host should materialize the current provenance/membership owner");
    runtime
        .materialize_designation("selectedFolder")
        .expect("host should materialize the selected child");
    assert_eq!(runtime.dormant_backing_keys().len(), 0);

    runtime
        .run_action("restoreFromTrash")
        .expect("remembered parent should feed transfer and structural restore directly");

    let mut provider = runtime.into_provider();
    assert_eq!(
        provider.loads.len(),
        3,
        "source restore should not perform any extra backing reads after explicit materialization"
    );
    let source_key = provider.loads[0].clone();
    let trash_key = provider.loads[1].clone();
    let selected_key = provider.loads[2].clone();
    assert_ne!(source_key, trash_key);
    assert_ne!(source_key, selected_key);
    assert_ne!(trash_key, selected_key);

    assert_eq!(
        provider.replacements.len(),
        1,
        "restore should publish one accepted candidate"
    );
    let replaced = &provider.replacements[0];
    assert_eq!(
        replaced.len(),
        3,
        "source membership, trash membership, and selected local slot should be the only changed dynamic backings"
    );
    assert!(replaced.contains(&source_key));
    assert!(replaced.contains(&trash_key));
    assert!(replaced.contains(&selected_key));

    provider.loads.clear();
    provider.replacements.clear();
    let mut restarted = PartialPersistentRuntime::open(checked, provider)
        .expect("restored world should restart with all dynamic folders dormant");
    assert_eq!(restarted.dormant_backing_keys().len(), 3);

    restarted
        .materialize_designation("sourceFolder")
        .expect("restored source owner should remain materializable");
    restarted
        .materialize_designation("trashFolder")
        .expect("trash owner should remain materializable");
    restarted
        .materialize_designation("selectedFolder")
        .expect("same selected child should remain materializable");

    assert_eq!(restarted.value("inSource").unwrap(), Value::Bool(true));
    assert_eq!(restarted.value("inTrash").unwrap(), Value::Bool(false));

    restarted
        .run_action("proveRestoredOwner")
        .expect("source should again prove selected lifetime provenance");
    restarted
        .run_action("inspectRememberedParent")
        .expect("cleared remembered parent should copy as absence");
    assert_eq!(
        restarted.value("restoreDestinationPresent").unwrap(),
        Value::Bool(false)
    );

    let provider = restarted.into_provider();
    assert_eq!(provider.loads.len(), 3);
    assert_eq!(provider.loads[0], source_key);
    assert_eq!(provider.loads[1], trash_key);
    assert_eq!(provider.loads[2], selected_key);
}
