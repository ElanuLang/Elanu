from pathlib import Path

runtime = Path("compiler/src/runtime.rs")
text = runtime.read_text()
anchor = "#[cfg(test)]\nmod undo_history_negative_evidence;\n"
addition = "#[cfg(test)]\nmod external_resource_negative_evidence;\n"
if text.count(anchor) != 1:
    raise SystemExit(f"expected runtime module anchor once, found {text.count(anchor)}")
if addition not in text:
    text = text.replace(anchor, anchor + addition, 1)
runtime.write_text(text)

Path("compiler/src/runtime/external_resource_negative_evidence.rs").write_text(r'''use super::persistence::{PersistenceImage, PersistenceProvider, PersistentRuntime};
use super::{RuntimeError, Value};
use crate::check_source_with_runtime_models;
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Default)]
struct CoordinatedStore {
    current_image: Option<Vec<u8>>,
    current_payload: Vec<u8>,
    pending_payload: Option<Vec<u8>>,
    reject_external_prepare: bool,
    reject_image_accept: bool,
    replace_attempts: usize,
}

#[derive(Clone)]
struct CoordinatingProvider {
    store: Rc<RefCell<CoordinatedStore>>,
}

impl CoordinatingProvider {
    fn new() -> (Self, Rc<RefCell<CoordinatedStore>>) {
        let store = Rc::new(RefCell::new(CoordinatedStore::default()));
        (
            Self {
                store: Rc::clone(&store),
            },
            store,
        )
    }
}

impl PersistenceProvider for CoordinatingProvider {
    fn load(&mut self) -> Result<Option<PersistenceImage>, RuntimeError> {
        self.store
            .borrow()
            .current_image
            .as_deref()
            .map(PersistenceImage::decode)
            .transpose()
    }

    fn replace(&mut self, image: &PersistenceImage) -> Result<(), RuntimeError> {
        let candidate_image = image.encode()?;
        let mut store = self.store.borrow_mut();
        store.replace_attempts += 1;

        let candidate_payload = store.pending_payload.take().ok_or_else(|| {
            RuntimeError::new("coordinated persistence replacement has no staged external payload")
        })?;

        if store.reject_external_prepare {
            store.reject_external_prepare = false;
            return Err(RuntimeError::new("external resource preparation failed"));
        }

        if store.reject_image_accept {
            store.reject_image_accept = false;
            return Err(RuntimeError::new("Elanu image acceptance failed"));
        }

        // The fake host transaction has one final acceptance step for both resources.
        // Until this point neither authoritative value has changed.
        store.current_image = Some(candidate_image);
        store.current_payload = candidate_payload;
        Ok(())
    }
}

fn run_coordinated_action(
    app: &mut PersistentRuntime<CoordinatingProvider>,
    store: &Rc<RefCell<CoordinatedStore>>,
    action: &str,
    candidate_payload: &[u8],
) -> Result<(), RuntimeError> {
    {
        let mut store = store.borrow_mut();
        if store.pending_payload.is_some() {
            return Err(RuntimeError::new(
                "external resource already has a pending candidate",
            ));
        }
        store.pending_payload = Some(candidate_payload.to_vec());
    }

    let result = app.run_action(action);
    if result.is_err() {
        store.borrow_mut().pending_payload = None;
    }
    result
}

const SOURCE: &str = r#"
state model Document {
    state title = ""
}

state model Folder {
    state documents: [live Document] = []
}

state workspace: Folder
state selected: maybe live Document = none

derived selectedTitle = selected.title

action seed {
    create Document in workspace as document {
        through document.title = "Draft"
        insert document into workspace.documents
    }
    selected = workspace.documents[0]
}

action updateMetadata {
    through selected.title = "Final"
}

action failedUpdate {
    through selected.title = "Wrong"
    fail "abort"
}
"#;

fn checked_source() -> crate::CheckedSource {
    check_source_with_runtime_models(SOURCE).expect("external-resource pressure source should check")
}

#[test]
fn transactional_host_resource_composes_with_existing_persistence_boundary() {
    let (provider, observer) = CoordinatingProvider::new();
    let mut app = PersistentRuntime::open(checked_source(), provider)
        .expect("coordinated persistent runtime should open");

    run_coordinated_action(&mut app, &observer, "seed", b"payload A")
        .expect("initial Document and payload should commit together");
    assert_eq!(
        app.value("selectedTitle").unwrap(),
        Value::String("Draft".to_string())
    );
    assert_eq!(observer.borrow().current_payload, b"payload A");
    assert!(observer.borrow().current_image.is_some());
    assert!(observer.borrow().pending_payload.is_none());

    let accepted_image = observer.borrow().current_image.clone();
    let accepted_payload = observer.borrow().current_payload.clone();
    let attempts_before_semantic_failure = observer.borrow().replace_attempts;

    run_coordinated_action(&mut app, &observer, "failedUpdate", b"payload wrong")
        .expect_err("semantic failure must not publish either candidate");
    assert_eq!(
        observer.borrow().replace_attempts,
        attempts_before_semantic_failure,
        "semantic failure must not reach provider replacement"
    );
    assert_eq!(observer.borrow().current_image, accepted_image);
    assert_eq!(observer.borrow().current_payload, accepted_payload);
    assert!(observer.borrow().pending_payload.is_none());
    assert_eq!(
        app.value("selectedTitle").unwrap(),
        Value::String("Draft".to_string())
    );

    observer.borrow_mut().reject_external_prepare = true;
    run_coordinated_action(&mut app, &observer, "updateMetadata", b"payload B")
        .expect_err("external preparation failure must reject the Elanu candidate too");
    assert_eq!(observer.borrow().current_image, accepted_image);
    assert_eq!(observer.borrow().current_payload, accepted_payload);
    assert!(observer.borrow().pending_payload.is_none());
    assert_eq!(
        app.value("selectedTitle").unwrap(),
        Value::String("Draft".to_string())
    );

    observer.borrow_mut().reject_image_accept = true;
    run_coordinated_action(&mut app, &observer, "updateMetadata", b"payload B")
        .expect_err("image acceptance failure must leave external payload prior too");
    assert_eq!(observer.borrow().current_image, accepted_image);
    assert_eq!(observer.borrow().current_payload, accepted_payload);
    assert!(observer.borrow().pending_payload.is_none());
    assert_eq!(
        app.value("selectedTitle").unwrap(),
        Value::String("Draft".to_string())
    );

    run_coordinated_action(&mut app, &observer, "updateMetadata", b"payload B")
        .expect("one host transaction should accept image and external payload together");
    assert_eq!(
        app.value("selectedTitle").unwrap(),
        Value::String("Final".to_string())
    );
    assert_eq!(observer.borrow().current_payload, b"payload B");
    assert_ne!(observer.borrow().current_image, accepted_image);
    assert!(observer.borrow().pending_payload.is_none());

    let provider = app.into_provider();
    let mut restarted = PersistentRuntime::open(checked_source(), provider)
        .expect("accepted Elanu image should restart normally");
    assert_eq!(restarted.derived_evaluations("selectedTitle"), Some(0));
    assert_eq!(
        restarted.value("selectedTitle").unwrap(),
        Value::String("Final".to_string())
    );
    assert_eq!(observer.borrow().current_payload, b"payload B");
}
''')
