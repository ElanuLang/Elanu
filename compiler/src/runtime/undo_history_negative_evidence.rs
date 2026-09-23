use super::persistence::{PersistenceImage, PersistenceProvider, PersistentRuntime};
use super::{RuntimeError, Value};
use crate::check_source_with_runtime_models;
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Default)]
struct HistoryStore {
    current: Option<Vec<u8>>,
    history: Vec<Vec<u8>>,
    reject_next_replace: bool,
    replace_attempts: usize,
}

#[derive(Clone)]
struct HistoryProvider {
    store: Rc<RefCell<HistoryStore>>,
}

impl HistoryProvider {
    fn new() -> (Self, Rc<RefCell<HistoryStore>>) {
        let store = Rc::new(RefCell::new(HistoryStore::default()));
        (
            Self {
                store: Rc::clone(&store),
            },
            store,
        )
    }

    fn rewind_one(&mut self) -> Result<(), RuntimeError> {
        let mut store = self.store.borrow_mut();
        let prior = store
            .history
            .pop()
            .ok_or_else(|| RuntimeError::new("no prior committed persistence image"))?;
        store.current = Some(prior);
        Ok(())
    }
}

impl PersistenceProvider for HistoryProvider {
    fn load(&mut self) -> Result<Option<PersistenceImage>, RuntimeError> {
        self.store
            .borrow()
            .current
            .as_deref()
            .map(PersistenceImage::decode)
            .transpose()
    }

    fn replace(&mut self, image: &PersistenceImage) -> Result<(), RuntimeError> {
        let bytes = image.encode()?;
        let mut store = self.store.borrow_mut();
        store.replace_attempts += 1;
        if store.reject_next_replace {
            store.reject_next_replace = false;
            return Err(RuntimeError::new("history provider rejected replacement"));
        }
        if let Some(prior) = store.current.replace(bytes) {
            store.history.push(prior);
        }
        Ok(())
    }
}

const SOURCE: &str = r#"
state model Document {
    state title = ""
}

state model Folder {
    state name = ""
    state folders: [live Folder] = []
    state documents: [live Document] = []
    state restoreParent: maybe live Folder = none
}

state workspace: Folder
state sourceFolder: maybe live Folder = none
state trashFolder: maybe live Folder = none
state selectedFolder: maybe live Folder = none
state restoreDestination: maybe live Folder = none

derived selectedInSource = selectedFolder is in sourceFolder.folders
derived selectedInTrash = selectedFolder is in trashFolder.folders
derived selectedName = selectedFolder.name
derived firstDocumentTitle = selectedFolder.documents[0].title

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

    create Folder in sourceFolder as folder {
        through folder.name = "Project"
        insert folder into sourceFolder.folders
    }
    selectedFolder = sourceFolder.folders[0]

    create Document in selectedFolder as document {
        through document.title = "Draft"
        insert document into selectedFolder.documents
    }
}

action moveToTrash {
    through selectedFolder.restoreParent = sourceFolder
    transfer selectedFolder from sourceFolder to trashFolder
    insert selectedFolder into trashFolder.folders
    remove selectedFolder from sourceFolder.folders
}

action failedRename {
    through selectedFolder.name = "Should Roll Back"
    fail "abort"
}

action restoreFromTrash {
    restoreDestination = selectedFolder.restoreParent
    transfer selectedFolder from trashFolder to restoreDestination
    insert selectedFolder into restoreDestination.folders
    remove selectedFolder from trashFolder.folders
    through selectedFolder.restoreParent = none
}

action staleOwnerProof {
    transfer selectedFolder from trashFolder to trashFolder
}
"#;

fn checked_source() -> crate::CheckedSource {
    check_source_with_runtime_models(SOURCE).expect("undo/history pressure source should check")
}

#[test]
fn opaque_provider_history_restores_prior_committed_world_without_inverse_logic() {
    let (provider, observer) = HistoryProvider::new();
    let mut app = PersistentRuntime::open(checked_source(), provider)
        .expect("history-backed persistent runtime should open");

    app.run_action("seed").expect("seed should commit durably");
    assert_eq!(observer.borrow().history.len(), 0);

    app.run_action("moveToTrash")
        .expect("move to Trash should commit durably");
    let moved_bytes = observer
        .borrow()
        .current
        .clone()
        .expect("moved world should be durable");
    assert_eq!(observer.borrow().history.len(), 1);
    assert_eq!(app.value("selectedInSource").unwrap(), Value::Bool(false));
    assert_eq!(app.value("selectedInTrash").unwrap(), Value::Bool(true));

    let history_len = observer.borrow().history.len();
    let current_before_rejection = observer.borrow().current.clone();
    let attempts_before_rejection = observer.borrow().replace_attempts;
    observer.borrow_mut().reject_next_replace = true;
    app.run_action("restoreFromTrash")
        .expect_err("provider rejection must not publish or archive a world");
    assert_eq!(observer.borrow().history.len(), history_len);
    assert_eq!(observer.borrow().current, current_before_rejection);
    assert_eq!(
        observer.borrow().replace_attempts,
        attempts_before_rejection + 1
    );
    assert_eq!(app.value("selectedInTrash").unwrap(), Value::Bool(true));

    let attempts_before_semantic_failure = observer.borrow().replace_attempts;
    app.run_action("failedRename")
        .expect_err("semantic failure must not become history");
    assert_eq!(
        observer.borrow().replace_attempts,
        attempts_before_semantic_failure
    );
    assert_eq!(observer.borrow().history.len(), history_len);
    assert_eq!(observer.borrow().current, current_before_rejection);

    let mut provider = app.into_provider();
    provider
        .rewind_one()
        .expect("one accepted prior world should be available");
    assert_eq!(provider.store.borrow().history.len(), 0);

    let mut undone = PersistentRuntime::open(checked_source(), provider)
        .expect("fresh runtime should restore rewound opaque image");
    assert_eq!(undone.derived_evaluations("selectedInSource"), Some(0));
    assert_eq!(undone.value("selectedInSource").unwrap(), Value::Bool(true));
    assert_eq!(undone.value("selectedInTrash").unwrap(), Value::Bool(false));
    assert_eq!(
        undone.value("selectedName").unwrap(),
        Value::String("Project".to_string())
    );
    assert_eq!(
        undone.value("firstDocumentTitle").unwrap(),
        Value::String("Draft".to_string())
    );

    undone
        .run_action("staleOwnerProof")
        .expect_err("Trash must no longer be the selected folder's lifetime owner after undo");
    undone
        .run_action("moveToTrash")
        .expect("the restored source-owner world should support the same move again");

    let provider = undone.into_provider();
    let repeated_moved_bytes = provider
        .store
        .borrow()
        .current
        .clone()
        .expect("repeated move should produce durable bytes");
    assert_eq!(
        repeated_moved_bytes, moved_bytes,
        "rewind + fresh restore + same action should reproduce the exact committed image"
    );
}
