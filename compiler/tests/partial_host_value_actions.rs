use std::collections::HashMap;

use elanu_compiler::{
    check_source_with_runtime_models,
    runtime::{
        persistence::partial::{PartialPersistenceProvider, PartialPersistentRuntime},
        RuntimeError, Value,
    },
};

const SOURCE: &str = r#"
state model Note {
    state title = ""
    state body = ""
}

state model Cabinet {
    state notes: [live Note] = []
}

state cabinet: Cabinet
state selectedNote: maybe live Note = none

derived selectedTitle = selectedNote.title
derived selectedBody = selectedNote.body

action seed {
    create Note in cabinet as note {
        through note.title = "Initial"
        through note.body = "Initial body"
        insert note into cabinet.notes
    }
    selectedNote = cabinet.notes[0]
}

action editSelected(title: String, body: String) {
    through selectedNote.title = title
    through selectedNote.body = body
}

action editThenFail(title: String) {
    through selectedNote.title = title
    fail "abort edit"
}
"#;

#[derive(Debug, Clone, Default)]
struct MemoryProvider {
    manifest: Option<Vec<u8>>,
    backing: HashMap<Vec<u8>, Vec<u8>>,
}

impl PartialPersistenceProvider for MemoryProvider {
    fn load_manifest(&mut self) -> Result<Option<Vec<u8>>, RuntimeError> {
        Ok(self.manifest.clone())
    }

    fn load_backing(&mut self, key: &[u8]) -> Result<Option<Vec<u8>>, RuntimeError> {
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
fn host_values_edit_selected_note_and_survive_restart() {
    let checked = check_source_with_runtime_models(SOURCE).expect("cabinet source should check");
    let mut runtime = PartialPersistentRuntime::open(checked.clone(), MemoryProvider::default())
        .expect("fresh partial runtime should open");

    runtime.run_action("seed").expect("seed should publish");
    runtime
        .run_action_with_values(
            "editSelected",
            &[
                Value::String("Cabinet title".into()),
                Value::String("Cabinet body".into()),
            ],
        )
        .expect("host values should bind ordinary action parameters");

    let provider = runtime.into_provider();
    let mut restarted = PartialPersistentRuntime::open(checked, provider)
        .expect("restart should leave the selected note dormant");
    assert_eq!(restarted.dormant_backing_keys().len(), 1);

    restarted
        .materialize_designation("selectedNote")
        .expect("host should explicitly materialize the selected note");
    assert_eq!(
        restarted.value("selectedTitle").unwrap(),
        Value::String("Cabinet title".into())
    );
    assert_eq!(
        restarted.value("selectedBody").unwrap(),
        Value::String("Cabinet body".into())
    );
}

#[test]
fn failed_host_value_action_preserves_prior_selected_note() {
    let checked = check_source_with_runtime_models(SOURCE).expect("cabinet source should check");
    let mut runtime = PartialPersistentRuntime::open(checked, MemoryProvider::default())
        .expect("fresh partial runtime should open");

    runtime.run_action("seed").expect("seed should publish");
    let error = runtime
        .run_action_with_values("editThenFail", &[Value::String("Must roll back".into())])
        .expect_err("explicit failure should reject host-driven edit");
    assert!(error.message.contains("abort edit"));
    assert_eq!(
        runtime.value("selectedTitle").unwrap(),
        Value::String("Initial".into())
    );
}

#[test]
fn host_value_boundary_rejects_identity_sequences() {
    let checked = check_source_with_runtime_models(SOURCE).expect("cabinet source should check");
    let mut runtime = PartialPersistentRuntime::open(checked, MemoryProvider::default())
        .expect("fresh partial runtime should open");

    let error = runtime
        .run_action_with_values(
            "editSelected",
            &[Value::Sequence {
                element_model: "Note".into(),
                targets: Vec::new(),
            }],
        )
        .expect_err("host must not inject modeled identity sequences");
    assert!(error.message.contains("identity sequences"));
}
