use std::collections::HashMap;

use elanu_compiler::{
    check_source_with_runtime_models,
    runtime::{
        persistence::partial::{PartialPersistenceProvider, PartialPersistentRuntime},
        RuntimeError, Value,
    },
};

const SOURCE: &str = r#"
state model Document {
    state title = ""
}

state model Folder {
    state documents: [live Document] = []
}

state folder: Folder
state selected: maybe live Document = none
state observedSelected = ""

action seed {
    create Document in folder as first {
        through first.title = "First"
        insert first into folder.documents
    }
    selected = folder.documents[0]
}

action rename(state target: String) {
    target = "Renamed"
}

action clearThenGrant {
    selected = none
    rename(state through selected.title)
}

action observeSelected {
    observedSelected = selected.title
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
fn staged_absence_blocks_designation_member_authority_and_rolls_back() {
    let checked = check_source_with_runtime_models(SOURCE)
        .expect("absent designation authority pressure source should check");
    let mut initial = PartialPersistentRuntime::open(checked.clone(), MemoryProvider::default())
        .expect("fresh partial runtime should open");
    initial.run_action("seed").expect("seed should publish");

    let mut provider = initial.into_provider();
    provider.loads.clear();
    provider.replacements.clear();
    assert_eq!(
        provider.backing.len(),
        1,
        "seed should produce one document backing"
    );

    let mut runtime = PartialPersistentRuntime::open(checked.clone(), provider)
        .expect("restart should leave the document dormant");
    assert_eq!(runtime.dormant_backing_keys().len(), 1);

    runtime
        .materialize_designation("selected")
        .expect("host should materialize the initially selected document");
    assert_eq!(runtime.dormant_backing_keys().len(), 0);

    let error = runtime
        .run_action("clearThenGrant")
        .expect_err("staged absence must make the subsequent writable grant fail");
    assert!(
        error.message.contains("no target"),
        "failure should report absent designation target, got: {}",
        error.message
    );

    let mut provider = runtime.into_provider();
    assert_eq!(
        provider.loads.len(),
        1,
        "failed source execution should not perform another dynamic backing read"
    );
    let selected_key = provider.loads[0].clone();
    assert!(
        provider.replacements.is_empty(),
        "failed transaction must not publish designation or child changes"
    );

    provider.loads.clear();
    provider.replacements.clear();
    let mut restarted = PartialPersistentRuntime::open(checked, provider)
        .expect("rollback should leave the committed designation and backing restartable");
    assert_eq!(restarted.dormant_backing_keys().len(), 1);

    restarted
        .materialize_designation("selected")
        .expect("rollback should restore the committed selected target");
    restarted
        .run_action("observeSelected")
        .expect("selected title should remain readable after rollback");
    assert_eq!(
        restarted.value("observedSelected").unwrap(),
        Value::String("First".into())
    );

    let provider = restarted.into_provider();
    assert_eq!(provider.loads, vec![selected_key]);
    assert!(provider.replacements.is_empty());
}
