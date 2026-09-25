use std::collections::HashMap;

use elanu_compiler::{
    check_source_with_runtime_models,
    runtime::{
        persistence::partial::{PartialPersistenceProvider, PartialPersistentRuntime},
        RuntimeError, Value,
    },
};

const SOURCE: &str = include_str!("../../examples/cabinet.elnu");

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

fn value(runtime: &mut PartialPersistentRuntime<MemoryProvider>, name: &str) -> Value {
    runtime.value(name).unwrap()
}

#[test]
fn cabinet_source_drives_create_edit_trash_restore_and_rollback() {
    let checked = check_source_with_runtime_models(SOURCE).expect("cabinet source should check");
    let mut runtime = PartialPersistentRuntime::open(checked.clone(), MemoryProvider::default())
        .expect("fresh cabinet should open");

    runtime
        .run_action("initialize")
        .expect("initialize should publish");
    assert_eq!(
        value(&mut runtime, "selectedFolderName"),
        Value::String("Notes".into())
    );

    runtime
        .run_action_with_values("createFolder", &[Value::String("Projects".into())])
        .expect("folder creation should publish");
    assert_eq!(
        value(&mut runtime, "selectedFolderName"),
        Value::String("Projects".into())
    );

    runtime
        .run_action_with_values(
            "createNote",
            &[
                Value::String("First note".into()),
                Value::String("Initial body".into()),
            ],
        )
        .expect("note creation should publish");
    assert_eq!(
        value(&mut runtime, "selectedNoteTitle"),
        Value::String("First note".into())
    );

    let error = runtime
        .run_action_with_values(
            "failEditSelectedNote",
            &[Value::String("Wrong title".into())],
        )
        .expect_err("failing cabinet edit should roll back");
    assert!(error.message.contains("abort cabinet edit"));
    assert_eq!(
        value(&mut runtime, "selectedNoteTitle"),
        Value::String("First note".into())
    );

    runtime
        .run_action("trashSelectedNote")
        .expect("trash move should publish");
    runtime.run_action("openTrash").expect("trash should open");
    runtime
        .run_action_with_values("selectNote", &[Value::Int(0)])
        .expect("trashed note should be selectable");
    assert_eq!(
        value(&mut runtime, "selectedNoteInTrash"),
        Value::Bool(true)
    );

    let provider = runtime.into_provider();
    let mut restarted = PartialPersistentRuntime::open(checked, provider)
        .expect("cabinet should restart with dynamic state dormant");
    restarted
        .materialize_designation("selectedFolder")
        .expect("visible trash folder should explicitly materialize");
    restarted
        .materialize_designation("selectedNote")
        .expect("visible selected note should explicitly materialize");
    assert_eq!(
        value(&mut restarted, "selectedNoteTitle"),
        Value::String("First note".into())
    );

    restarted
        .run_action("restoreSelectedNote")
        .expect("restore should publish");
    assert_eq!(
        value(&mut restarted, "selectedFolderName"),
        Value::String("Projects".into())
    );
    assert_eq!(
        value(&mut restarted, "selectedNoteTitle"),
        Value::String("First note".into())
    );
    assert_eq!(
        value(&mut restarted, "selectedNoteInTrash"),
        Value::Bool(false)
    );
}
