use std::collections::HashMap;

use elanu_compiler::{
    check_source_with_runtime_models,
    runtime::{
        persistence::partial::{PartialPersistenceProvider, PartialPersistentRuntime},
        RuntimeError,
    },
};

const SOURCE: &str = r#"
state model Document {
    state title = ""
}

state model Folder {
    state documents: [live Document] = []
    state pinnedDocument: maybe live Document = none
}

state model Workspace {
    state folders: [live Folder] = []
}

state workspace: Workspace
state selectedFolder: maybe live Folder = none
state otherFolder: maybe live Folder = none
state selectedDocument: maybe live Document = none
state observedPinned: maybe live Document = none

action seed {
    create Folder in workspace as firstFolder {
        insert firstFolder into workspace.folders
    }
    create Folder in workspace as secondFolder {
        insert secondFolder into workspace.folders
    }

    selectedFolder = workspace.folders[0]
    otherFolder = workspace.folders[1]

    create Document in selectedFolder as document {
        through document.title = "Pinned remotely"
        insert document into selectedFolder.documents
    }
    selectedDocument = selectedFolder.documents[0]
    through otherFolder.pinnedDocument = selectedDocument
}

action destroySelected {
    destroy selectedDocument in selectedFolder
}

action inspectPinned {
    observedPinned = otherFolder.pinnedDocument
}
"#;

#[derive(Debug, Clone, Default)]
struct MemoryProvider {
    manifest: Option<Vec<u8>>,
    backing: HashMap<Vec<u8>, Vec<u8>>,
    loads: Vec<Vec<u8>>,
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
        Ok(())
    }
}

#[test]
fn destroy_must_clear_model_local_optional_designation_owned_by_dormant_foreign_folder() {
    let checked = check_source_with_runtime_models(SOURCE)
        .expect("dormant model-local designation pressure source should check");
    let mut initial = PartialPersistentRuntime::open(checked.clone(), MemoryProvider::default())
        .expect("fresh partial runtime should open");
    initial.run_action("seed").expect("seed should publish");

    let mut provider = initial.into_provider();
    provider.loads.clear();
    let mut runtime = PartialPersistentRuntime::open(checked.clone(), provider)
        .expect("restart should open without dynamic backing reads");
    assert_eq!(
        runtime.dormant_backing_keys().len(),
        3,
        "both Folders and the selected Document should begin dormant"
    );
    assert!(runtime.into_provider().loads.is_empty());

    let mut provider = initial_provider_from_seed(checked.clone());
    provider.loads.clear();
    let mut runtime = PartialPersistentRuntime::open(checked.clone(), provider)
        .expect("second restart should open without dynamic backing reads");
    runtime
        .materialize_root_member_index("workspace", "folders", 0)
        .expect("lifetime-owner Folder should materialize");
    runtime
        .run_action("destroySelected")
        .expect("destroy should satisfy model-local optional-designation cleanup");
    assert_eq!(
        runtime.dormant_backing_keys().len(),
        1,
        "destroyed Document should cease to be backed while foreign Folder stays dormant"
    );

    let mut provider = runtime.into_provider();
    provider.loads.clear();
    let mut restarted = PartialPersistentRuntime::open(checked, provider)
        .expect("published post-destroy world should restart");
    restarted
        .materialize_designation("otherFolder")
        .expect("foreign Folder should remain live and independently materializable");
    restarted
        .run_action("inspectPinned")
        .expect("cleared model-local optional designation should copy as none");
    let error = restarted
        .materialize_designation("observedPinned")
        .expect_err("terminated target must leave the copied optional designation absent");
    assert!(
        error.message.contains("has no target"),
        "model-local optional designation must not retain the terminated identity: {}",
        error.message
    );
}

fn initial_provider_from_seed(checked: elanu_compiler::CheckedSource) -> MemoryProvider {
    let mut initial = PartialPersistentRuntime::open(checked, MemoryProvider::default())
        .expect("fresh partial runtime should open");
    initial.run_action("seed").expect("seed should publish");
    initial.into_provider()
}
