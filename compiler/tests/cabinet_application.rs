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
        .run_action_with_values("createFolder", &[Value::String("Archive".into())])
        .expect("nested folder creation should publish");
    assert_eq!(
        value(&mut runtime, "selectedFolderName"),
        Value::String("Archive".into())
    );

    runtime
        .run_action("selectHome")
        .expect("home selection should publish");
    assert_eq!(
        runtime
            .designation_member_len("selectedFolder", "folders")
            .expect("home folders should be observable"),
        1
    );
    assert_eq!(
        runtime
            .designation_member_index_value("selectedFolder", "folders", 0, "name")
            .expect("Projects row should be observable"),
        Value::String("Projects".into())
    );

    runtime
        .run_action_with_values("selectFolder", &[Value::Int(0)])
        .expect("Projects selection should publish");
    assert_eq!(
        value(&mut runtime, "selectedFolderName"),
        Value::String("Projects".into())
    );
    assert_eq!(
        runtime
            .designation_member_len("selectedFolder", "folders")
            .expect("Projects child folders should be observable"),
        1
    );
    assert_eq!(
        runtime
            .designation_member_index_value("selectedFolder", "folders", 0, "name")
            .expect("nested Archive row should be observable"),
        Value::String("Archive".into())
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
    assert_eq!(
        runtime
            .designation_member_len("selectedFolder", "notes")
            .expect("selected folder notes should be observable"),
        1
    );
    assert_eq!(
        runtime
            .designation_member_index_value("selectedFolder", "notes", 0, "title")
            .expect("visible note title should be observable"),
        Value::String("First note".into())
    );

    runtime
        .run_action_with_values(
            "editSelectedNote",
            &[
                Value::String("Edited note".into()),
                Value::String("Edited body".into()),
            ],
        )
        .expect("note edit should publish");
    assert_eq!(
        value(&mut runtime, "selectedNoteTitle"),
        Value::String("Edited note".into())
    );
    assert_eq!(
        value(&mut runtime, "selectedNoteBody"),
        Value::String("Edited body".into())
    );
    assert_eq!(
        runtime
            .designation_member_index_value("selectedFolder", "notes", 0, "title")
            .expect("edited visible note title should be observable"),
        Value::String("Edited note".into())
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
        Value::String("Edited note".into())
    );

    runtime
        .run_action("trashSelectedNote")
        .expect("trash move should publish");
    assert_eq!(
        value(&mut runtime, "selectedNoteInTrash"),
        Value::Bool(true)
    );

    let provider = runtime.into_provider();
    let mut restarted = PartialPersistentRuntime::open(checked.clone(), provider)
        .expect("cabinet should restart with dynamic state dormant");
    restarted
        .materialize_designation("selectedFolder")
        .expect("visible selected folder should explicitly materialize");
    restarted
        .materialize_designation("trashFolder")
        .expect("trash navigation should explicitly materialize");
    restarted
        .materialize_designation("selectedNote")
        .expect("persisted selected note should explicitly materialize");
    assert_eq!(
        value(&mut restarted, "selectedNoteTitle"),
        Value::String("Edited note".into())
    );
    assert_eq!(
        value(&mut restarted, "selectedNoteBody"),
        Value::String("Edited body".into())
    );
    assert_eq!(
        value(&mut restarted, "selectedNoteInTrash"),
        Value::Bool(true)
    );

    restarted
        .run_action("openTrash")
        .expect("trash navigation should publish");
    assert_eq!(
        value(&mut restarted, "selectedFolderName"),
        Value::String("Trash".into())
    );
    assert_eq!(
        value(&mut restarted, "selectedNotePresent"),
        Value::Bool(false)
    );
    assert_eq!(
        restarted
            .designation_member_len("selectedFolder", "notes")
            .expect("trash notes should be observable"),
        1
    );
    assert_eq!(
        restarted
            .designation_member_index_value("selectedFolder", "notes", 0, "title")
            .expect("trashed note row should be observable"),
        Value::String("Edited note".into())
    );

    restarted
        .run_action_with_values("selectNote", &[Value::Int(0)])
        .expect("trashed note selection should publish");
    restarted
        .materialize_designation("selectedNote")
        .expect("selected trashed note should explicitly materialize");
    assert_eq!(
        value(&mut restarted, "selectedNoteInTrash"),
        Value::Bool(true)
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
        Value::String("Edited note".into())
    );
    assert_eq!(
        value(&mut restarted, "selectedNoteInTrash"),
        Value::Bool(false)
    );

    restarted
        .run_action("trashSelectedNote")
        .expect("second trash move should publish");
    restarted
        .run_action("openTrash")
        .expect("trash should open before permanent deletion");
    restarted
        .run_action_with_values("selectNote", &[Value::Int(0)])
        .expect("trashed note should be selectable before permanent deletion");
    restarted
        .materialize_designation("selectedNote")
        .expect("trashed note should materialize before permanent deletion");
    restarted
        .run_action("permanentlyDeleteSelectedNote")
        .expect("permanent deletion should publish");
    assert_eq!(
        value(&mut restarted, "selectedNotePresent"),
        Value::Bool(false)
    );
    assert_eq!(
        restarted
            .designation_member_len("trashFolder", "notes")
            .expect("trash membership should remain observable after deletion"),
        0
    );

    let provider = restarted.into_provider();
    let mut reopened = PartialPersistentRuntime::open(checked, provider)
        .expect("cabinet should reopen after permanent deletion");
    reopened
        .materialize_designation("trashFolder")
        .expect("trash should explicitly materialize after restart");
    assert_eq!(
        value(&mut reopened, "selectedNotePresent"),
        Value::Bool(false)
    );
    assert_eq!(
        reopened
            .designation_member_len("trashFolder", "notes")
            .expect("trash should remain empty after restart"),
        0
    );
}
