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
state descendantFolder: maybe live Folder = none
state restoreDestination: maybe live Folder = none
state observedDescendantName = ""

derived selectedInSource = selectedFolder is in sourceFolder.folders
derived selectedInTrash = selectedFolder is in trashFolder.folders
derived descendantInSelected = descendantFolder is in selectedFolder.folders

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

action addDescendant {
    create Folder in selectedFolder as descendant {
        through descendant.name = "Descendant"
        insert descendant into selectedFolder.folders
    }
    descendantFolder = selectedFolder.folders[0]
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

action proveSelectedOwner {
    transfer selectedFolder from sourceFolder to sourceFolder
}

action proveDescendantOwner {
    transfer descendantFolder from selectedFolder to selectedFolder
}

action observeDescendant {
    observedDescendantName = descendantFolder.name
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
fn nonleaf_remembered_parent_restore_leaves_descendant_dormant_and_untouched() {
    let checked = check_source_with_runtime_models(SOURCE)
        .expect("non-leaf remembered-parent restore pressure source should check");
    let mut initial = PartialPersistentRuntime::open(checked.clone(), MemoryProvider::default())
        .expect("fresh partial runtime should open");
    initial.run_action("seed").expect("seed should publish");
    initial
        .run_action("addDescendant")
        .expect("committed descendant should publish");
    initial
        .run_action("moveToTrash")
        .expect("non-leaf selected root should move to trash before restart");

    let mut provider = initial.into_provider();
    provider.loads.clear();
    provider.replacements.clear();
    assert_eq!(
        provider.backing.len(),
        4,
        "source, trash, selected, and descendant should have distinct backings"
    );

    let mut runtime = PartialPersistentRuntime::open(checked.clone(), provider)
        .expect("restart should leave all dynamic folders dormant");
    assert_eq!(runtime.dormant_backing_keys().len(), 4);

    runtime
        .materialize_designation("sourceFolder")
        .expect("host should materialize source owner");
    runtime
        .materialize_designation("trashFolder")
        .expect("host should materialize trash owner");
    runtime
        .materialize_designation("selectedFolder")
        .expect("host should materialize moved non-leaf root");
    assert_eq!(
        runtime.dormant_backing_keys().len(),
        1,
        "descendant must remain dormant before restore"
    );

    runtime
        .run_action("restoreFromTrash")
        .expect("root restore should not require descendant materialization");

    let mut provider = runtime.into_provider();
    assert_eq!(
        provider.loads.len(),
        3,
        "restore must not read the dormant descendant backing"
    );
    let source_key = provider.loads[0].clone();
    let trash_key = provider.loads[1].clone();
    let selected_key = provider.loads[2].clone();

    assert_eq!(provider.replacements.len(), 1);
    let replaced = &provider.replacements[0];
    assert_eq!(
        replaced.len(),
        3,
        "only source, trash, and selected backings should change during root restore"
    );
    assert!(replaced.contains(&source_key));
    assert!(replaced.contains(&trash_key));
    assert!(replaced.contains(&selected_key));

    provider.loads.clear();
    provider.replacements.clear();
    let mut restarted = PartialPersistentRuntime::open(checked, provider)
        .expect("restored non-leaf world should restart dormant");
    assert_eq!(restarted.dormant_backing_keys().len(), 4);

    restarted
        .materialize_designation("sourceFolder")
        .expect("source should remain materializable");
    restarted
        .materialize_designation("trashFolder")
        .expect("trash should remain materializable");
    restarted
        .materialize_designation("selectedFolder")
        .expect("selected root should remain materializable");

    assert_eq!(
        restarted.value("selectedInSource").unwrap(),
        Value::Bool(true)
    );
    assert_eq!(
        restarted.value("selectedInTrash").unwrap(),
        Value::Bool(false)
    );
    restarted
        .run_action("proveSelectedOwner")
        .expect("source should again prove selected root provenance");

    let provider = restarted.into_provider();
    assert_eq!(
        provider.loads.len(),
        3,
        "verification before descendant access should still leave descendant dormant"
    );
    assert_eq!(provider.loads[0], source_key);
    assert_eq!(provider.loads[1], trash_key);
    assert_eq!(provider.loads[2], selected_key);

    let mut restarted = PartialPersistentRuntime::open(checked, provider)
        .expect("second restart should again leave descendant dormant");
    restarted
        .materialize_designation("selectedFolder")
        .expect("selected owner should materialize before descendant verification");
    restarted
        .materialize_designation("descendantFolder")
        .expect("descendant should load only on this explicit host request");
    assert_eq!(
        restarted.value("descendantInSelected").unwrap(),
        Value::Bool(true)
    );
    restarted
        .run_action("proveDescendantOwner")
        .expect("descendant provenance should remain rooted in selected");
    restarted
        .run_action("observeDescendant")
        .expect("descendant payload should remain unchanged");
    assert_eq!(
        restarted.value("observedDescendantName").unwrap(),
        Value::String("Descendant".into())
    );
}
