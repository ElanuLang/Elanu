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
state observedFirst = ""
state observedSelected = ""

action seed {
    create Document in folder as first {
        through first.title = "First"
        insert first into folder.documents
    }
    create Document in folder as second {
        through second.title = "Second"
        insert second into folder.documents
    }
    selected = folder.documents[0]
}

action retargetThenRename(state target: String) {
    selected = folder.documents[1]
    target = "Renamed"
}

action grantThenRetarget {
    retargetThenRename(state through selected.title)
}

action observeFirst {
    observedFirst = folder.documents[0].title
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
fn granted_designation_member_authority_remains_captured_after_reselection() {
    let checked = check_source_with_runtime_models(SOURCE)
        .expect("designation authority capture pressure source should check");
    let mut initial = PartialPersistentRuntime::open(checked.clone(), MemoryProvider::default())
        .expect("fresh partial runtime should open");
    initial.run_action("seed").expect("seed should publish");

    let mut provider = initial.into_provider();
    provider.loads.clear();
    provider.replacements.clear();
    assert_eq!(
        provider.backing.len(),
        2,
        "seed should produce two document backings"
    );

    let mut runtime = PartialPersistentRuntime::open(checked.clone(), provider)
        .expect("restart should leave both documents dormant");
    assert_eq!(runtime.dormant_backing_keys().len(), 2);

    runtime
        .materialize_designation("selected")
        .expect("host should materialize the initially selected first document");
    runtime
        .materialize_root_member_index("folder", "documents", 1)
        .expect("host should materialize the second document before source execution");
    assert_eq!(
        runtime.dormant_backing_keys().len(),
        0,
        "both candidate targets should already be resident"
    );

    runtime
        .run_action("grantThenRetarget")
        .expect("later designation reselection must not retarget already-granted authority");

    let mut provider = runtime.into_provider();
    assert_eq!(
        provider.loads.len(),
        2,
        "source execution should not perform any additional dynamic backing reads"
    );
    let first_key = provider.loads[0].clone();
    let second_key = provider.loads[1].clone();
    assert_ne!(
        first_key, second_key,
        "the two children must have distinct backing identities"
    );
    assert_eq!(
        provider.replacements,
        vec![vec![first_key.clone()]],
        "publication should replace only the originally granted first child's backing"
    );

    provider.loads.clear();
    provider.replacements.clear();
    let mut restarted = PartialPersistentRuntime::open(checked, provider)
        .expect("published authority capture mutation should restart with both documents dormant");
    assert_eq!(restarted.dormant_backing_keys().len(), 2);

    restarted
        .materialize_root_member_index("folder", "documents", 0)
        .expect("first child should remain independently materializable");
    restarted
        .run_action("observeFirst")
        .expect("captured first title should be readable after restart");
    assert_eq!(
        restarted.value("observedFirst").unwrap(),
        Value::String("Renamed".into())
    );

    restarted
        .materialize_designation("selected")
        .expect("persisted reselection should now materialize the second child");
    restarted
        .run_action("observeSelected")
        .expect("newly selected second title should be readable after restart");
    assert_eq!(
        restarted.value("observedSelected").unwrap(),
        Value::String("Second".into())
    );

    let provider = restarted.into_provider();
    assert_eq!(provider.loads.len(), 2);
    assert_eq!(provider.loads[0], first_key);
    assert_eq!(provider.loads[1], second_key);
}
