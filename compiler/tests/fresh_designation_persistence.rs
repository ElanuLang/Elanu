use elanu_compiler::{
    check_source_with_runtime_models,
    runtime::{Runtime, Value},
};

// Contract: creation scope may persist one exact fresh child identity into compatible
// persistent designation state without turning that identity into an ordinary value.
// The fresh identity has designation meaning only in that compatible assignment context;
// existing generated insertion/lifetime uses keep treating the private carrier as opaque.
// Model metadata is compiler-owned side information rather than a public value encoding.
// General lexical capture persistence remains a separate, unselected semantic question.
const SOURCE: &str = r#"
state model Document {
    state title = ""
}

state model Workspace {
    state documents: [live Document] = []
}

state workspace: Workspace
state initialDocument: Document
state selected: maybe live Document = none
state pinned: live Document = live initialDocument

action createAndSelect {
    create Document in workspace as document {
        through document.title = "New"
        insert document into workspace.documents
        selected = document
        pinned = document
    }
}

action createSelectThenFail {
    create Document in workspace as document {
        insert document into workspace.documents
        selected = document
        fail "rollback"
    }
}
"#;

fn runtime() -> Runtime {
    let checked = check_source_with_runtime_models(SOURCE)
        .expect("fresh scoped designation should persist into compatible live state");
    Runtime::from_checked_source(&checked).expect("runtime should initialize")
}

fn document_targets(runtime: &mut Runtime) -> Vec<String> {
    let Value::Sequence { targets, .. } = runtime
        .value("__elanu_mseq$workspace$documents")
        .expect("documents membership should exist")
    else {
        panic!("documents should be a runtime sequence");
    };
    targets
}

#[test]
fn fresh_scoped_identity_can_be_persisted_without_reconstructing_it() {
    let mut runtime = runtime();
    runtime
        .run_action("createAndSelect")
        .expect("create-and-select should commit");

    let targets = document_targets(&mut runtime);
    assert_eq!(targets.len(), 1);
    let created = targets[0].clone();

    assert_eq!(
        runtime.value("__elanu_live$selected").unwrap(),
        Value::String(created.clone())
    );
    assert_eq!(
        runtime.value("__elanu_live$pinned").unwrap(),
        Value::String(created)
    );
}

#[test]
fn fresh_designation_persistence_rolls_back_with_the_creating_action() {
    let mut runtime = runtime();
    let before = runtime.value("__elanu_live$selected").unwrap();

    runtime
        .run_action("createSelectThenFail")
        .expect_err("later failure should roll back creation, insertion, and selection");

    assert!(document_targets(&mut runtime).is_empty());
    assert_eq!(runtime.value("__elanu_live$selected").unwrap(), before);
}

#[test]
fn fresh_designation_persistence_rejects_incompatible_target_model() {
    let source = r#"
state model Document { state title = "" }
state model Project { state name = "" }
state model Workspace { state documents: [live Document] = [] }
state workspace: Workspace
state selectedProject: maybe live Project = none

action attempt {
    create Document in workspace as document {
        selectedProject = document
    }
}
"#;

    let errors = check_source_with_runtime_models(source)
        .expect_err("fresh Document identity must not flow into maybe live Project");
    assert!(errors.iter().any(|error| {
        error.message.contains("Document")
            && error.message.contains("Project")
            && error.message.contains("designation")
    }));
}

#[test]
fn fresh_designation_identity_cannot_leak_into_ordinary_string_state() {
    let source = r#"
state model Document { state title = "" }
state model Workspace { state documents: [live Document] = [] }
state workspace: Workspace
state leaked = ""

action attempt {
    create Document in workspace as document {
        leaked = document
    }
}
"#;

    let errors = check_source_with_runtime_models(source)
        .expect_err("private identity transport must not become an ordinary String value");
    assert!(errors.iter().any(|error| {
        error.message.contains("scoped live designation")
            && error.message.contains("persistent live")
    }));
}
