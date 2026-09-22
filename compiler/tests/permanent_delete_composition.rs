use elanu_compiler::{
    check_source_with_runtime_models,
    runtime::{Runtime, Value},
};

// Composition pressure: a realistic soft-delete / restore / permanent-delete workflow
// should not require an application tombstone, business-key lookup, mirrored liveness,
// or manual enumeration of every membership/designation that still names the child.
const SOURCE: &str = r#"
state model Document {
    state title = ""
}

state model Workspace {
    state active: [live Document] = []
    state trash: [live Document] = []
}

state model SearchIndex {
    state cached: [live Document] = []
}

state workspace: Workspace
state searchIndex: SearchIndex
state selected: maybe live Document = none
state recent: maybe live Document = none

action seed {
    create Document in workspace as first {
        through first.title = "First"
        insert first into workspace.active
        insert first into searchIndex.cached
        insert first into searchIndex.cached
    }
    create Document in workspace as second {
        through second.title = "Second"
        insert second into workspace.active
    }
    selected = workspace.active[0]
    recent = workspace.active[0]
}

action softDeleteSelected {
    remove selected from workspace.active
    insert selected into workspace.trash
}

action restoreSelected {
    remove selected from workspace.trash
    insert selected into workspace.active
}

action permanentlyDeleteSelected {
    destroy selected in workspace
}
"#;

fn runtime() -> Runtime {
    let checked =
        check_source_with_runtime_models(SOURCE).expect("composition source should check");
    Runtime::from_checked_source(&checked).expect("runtime should initialize")
}

fn targets(runtime: &mut Runtime, name: &str) -> Vec<String> {
    let Value::Sequence { targets, .. } = runtime.value(name).expect("sequence should exist")
    else {
        panic!("{name} should be a runtime sequence");
    };
    targets
}

#[test]
fn realistic_trash_lifecycle_composes_without_mirrored_liveness_or_manual_cleanup() {
    let mut runtime = runtime();
    runtime.run_action("seed").expect("seed should commit");

    let initial_active = targets(&mut runtime, "__meld_mseq$workspace$active");
    assert_eq!(initial_active.len(), 2);
    let first = initial_active[0].clone();
    let second = initial_active[1].clone();
    assert_eq!(
        targets(&mut runtime, "__meld_mseq$searchIndex$cached"),
        vec![first.clone(), first.clone()]
    );

    runtime
        .run_action("softDeleteSelected")
        .expect("soft delete should edit membership without ending child lifetime");

    assert_eq!(
        targets(&mut runtime, "__meld_mseq$workspace$active"),
        vec![second.clone()]
    );
    assert_eq!(
        targets(&mut runtime, "__meld_mseq$workspace$trash"),
        vec![first.clone()]
    );
    assert_eq!(
        runtime.value("__meld_live$selected").unwrap(),
        Value::String(first.clone())
    );
    assert_eq!(
        runtime.value("__meld_live$recent").unwrap(),
        Value::String(first.clone())
    );

    runtime
        .run_action("restoreSelected")
        .expect("restore should reuse the same persistent child identity");
    assert!(targets(&mut runtime, "__meld_mseq$workspace$trash").is_empty());
    assert_eq!(
        targets(&mut runtime, "__meld_mseq$workspace$active"),
        vec![second.clone(), first.clone()]
    );

    runtime
        .run_action("softDeleteSelected")
        .expect("the same identity should be soft-deletable again");
    runtime
        .run_action("permanentlyDeleteSelected")
        .expect("root owner should permanently delete the committed leaf child");

    assert_eq!(
        targets(&mut runtime, "__meld_mseq$workspace$active"),
        vec![second]
    );
    assert!(targets(&mut runtime, "__meld_mseq$workspace$trash").is_empty());
    assert!(targets(&mut runtime, "__meld_mseq$searchIndex$cached").is_empty());
    assert_eq!(
        runtime.value("__meld_live$selected").unwrap(),
        Value::String(String::new())
    );
    assert_eq!(
        runtime.value("__meld_live$recent").unwrap(),
        Value::String(String::new())
    );
}
