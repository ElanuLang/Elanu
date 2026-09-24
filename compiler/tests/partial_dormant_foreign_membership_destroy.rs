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
}

state model Workspace {
    state folders: [live Folder] = []
}

state workspace: Workspace
state selectedFolder: maybe live Folder = none
state otherFolder: maybe live Folder = none
state selectedDocument: maybe live Document = none

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
        through document.title = "Shared structurally"
        insert document into selectedFolder.documents
        insert document into otherFolder.documents
    }
    selectedDocument = selectedFolder.documents[0]
}

action destroySelected {
    destroy selectedDocument in selectedFolder
}

action selectFirstOther {
    selectedDocument = otherFolder.documents[0]
}
"#;

#[derive(Debug, Clone, Default)]
struct MemoryProvider {
    manifest: Option<Vec<u8>>,
    backing: HashMap<Vec<u8>, Vec<u8>>,
    loads: Vec<Vec<u8>>,
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
        Ok(())
    }
}

fn checked() -> elanu_compiler::CheckedSource {
    check_source_with_runtime_models(SOURCE)
        .expect("cross-owner dormant cleanup pressure source should check")
}

fn seeded_provider() -> (elanu_compiler::CheckedSource, MemoryProvider) {
    let checked = checked();
    let mut initial = PartialPersistentRuntime::open(checked.clone(), MemoryProvider::default())
        .expect("fresh partial runtime should open");
    initial.run_action("seed").expect("seed should publish");
    (checked, initial.into_provider())
}

fn assert_foreign_membership_is_clean(
    checked: elanu_compiler::CheckedSource,
    mut provider: MemoryProvider,
    expected_foreign_key: Vec<u8>,
) {
    provider.loads.clear();
    let mut restarted = PartialPersistentRuntime::open(checked, provider)
        .expect("published post-destroy world should restart");
    restarted
        .materialize_designation("otherFolder")
        .expect("foreign Folder should remain live and independently materializable");
    restarted
        .run_action("selectFirstOther")
        .expect_err("destroy semantics require the terminated child occurrence to be absent from the formerly dormant foreign membership");

    let provider = restarted.into_provider();
    assert_eq!(
        provider.loads,
        vec![expected_foreign_key],
        "post-destroy check should materialize only the surviving foreign Folder"
    );
}

#[test]
fn destroy_cleans_occurrence_owned_by_still_dormant_foreign_folder() {
    let (checked, mut provider) = seeded_provider();
    provider.loads.clear();
    let mut runtime = PartialPersistentRuntime::open(checked.clone(), provider)
        .expect("restart should open without dynamic backing reads");
    assert_eq!(
        runtime.dormant_backing_keys().len(),
        3,
        "both Folders and the shared Document should begin dormant"
    );

    runtime
        .materialize_root_member_index("workspace", "folders", 0)
        .expect("lifetime-owner Folder should materialize");
    runtime
        .run_action("destroySelected")
        .expect("destroy should clean structural occurrences across dormant backing");

    let dormant = runtime.dormant_backing_keys();
    assert_eq!(
        dormant.len(),
        1,
        "after destroy only the still-dormant foreign Folder should remain backed and dormant"
    );
    let foreign_key = dormant[0].clone();
    let provider = runtime.into_provider();
    assert_eq!(
        provider.loads.len(),
        2,
        "destroy should add exactly one targeted dormant-backing read after explicit owner materialization"
    );
    assert_eq!(
        provider.loads.last(),
        Some(&foreign_key),
        "the targeted private read must be the Folder that remains dormant, not the destroyed Document"
    );

    assert_foreign_membership_is_clean(checked, provider, foreign_key);
}

#[test]
fn rejected_dormant_foreign_cleanup_preserves_prior_world_and_can_retry() {
    let (checked, mut provider) = seeded_provider();
    let manifest_before = provider.manifest.clone();
    let backing_before = provider.backing.clone();
    provider.loads.clear();
    provider.reject_next = true;

    let mut runtime = PartialPersistentRuntime::open(checked.clone(), provider)
        .expect("restart should open without dynamic backing reads");
    runtime
        .materialize_root_member_index("workspace", "folders", 0)
        .expect("lifetime-owner Folder should materialize");
    runtime
        .run_action("destroySelected")
        .expect_err("provider rejection must reject the dormant cleanup candidate atomically");

    assert_eq!(
        runtime.dormant_backing_keys().len(),
        2,
        "rejected publication must keep both the foreign Folder and Document live/dormant"
    );
    let provider = runtime.into_provider();
    assert_eq!(provider.manifest, manifest_before);
    assert_eq!(provider.backing, backing_before);
    assert_eq!(
        provider.loads.len(),
        2,
        "rejected candidate may read only the explicit owner and one affected dormant Folder"
    );

    let mut retry = PartialPersistentRuntime::open(checked.clone(), provider)
        .expect("prior durable world should remain restartable after rejection");
    retry
        .materialize_root_member_index("workspace", "folders", 0)
        .expect("lifetime-owner Folder should materialize on retry");
    retry
        .run_action("destroySelected")
        .expect("same destroy should succeed after provider accepts candidate");
    let dormant = retry.dormant_backing_keys();
    assert_eq!(dormant.len(), 1);
    let foreign_key = dormant[0].clone();
    let provider = retry.into_provider();

    assert_foreign_membership_is_clean(checked, provider, foreign_key);
}
