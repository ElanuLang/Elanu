use super::*;
use crate::check_source_with_runtime_models;

fn runtime(source: &str) -> Runtime {
    let checked = check_source_with_runtime_models(source).expect("source should check");
    Runtime::from_checked_source(&checked).expect("runtime should initialize")
}

#[test]
fn committed_dynamic_identity_preserves_model_type_without_name_decoding() {
    let mut runtime = runtime(
        r#"
state model Folder {
    state name = ""
}

state workspace: Folder
state selected: maybe live Folder = none

action seed {
    create Folder in workspace as folder {
        selected = folder
    }
}

action deleteSelected {
    destroy selected in workspace
}
"#,
    );

    runtime.run_action("seed").expect("seed should commit");

    assert_eq!(runtime.dynamic_model_types.len(), 1);
    assert_eq!(
        runtime
            .dynamic_model_types
            .values()
            .next()
            .map(String::as_str),
        Some("Folder")
    );

    runtime
        .run_action("deleteSelected")
        .expect("destroy should commit");
    assert!(runtime.dynamic_model_types.is_empty());
}

#[test]
fn failed_creation_does_not_publish_dynamic_model_type() {
    let mut runtime = runtime(
        r#"
state model Folder {
    state name = ""
}

state workspace: Folder

action createThenFail {
    create Folder in workspace
    fail "abort"
}
"#,
    );

    runtime
        .run_action("createThenFail")
        .expect_err("failed action should roll creation back");

    assert!(runtime.dynamic_model_types.is_empty());
    assert!(runtime.dynamic_model_owners.is_empty());
}

#[test]
fn distinct_dynamic_models_keep_distinct_structured_model_types() {
    let mut runtime = runtime(
        r#"
state model Folder {
    state name = ""
}

state model Document {
    state title = ""
}

state workspace: Folder

action seed {
    create Folder in workspace
    create Document in workspace
}
"#,
    );

    runtime.run_action("seed").expect("seed should commit");

    let mut models = runtime
        .dynamic_model_types
        .values()
        .cloned()
        .collect::<Vec<_>>();
    models.sort();
    assert_eq!(models, vec!["Document".to_string(), "Folder".to_string()]);
}
