use elanu_compiler::{
    check_source_with_runtime_models,
    runtime::{Runtime, Value},
};

// Source-contract regression: `destroy designation in owner` ends rooted child lifetime;
// it is not structural removal and does not make designation or membership owning.
// Root provenance is the lifetime-authority fact; foreign membership is not authority.
const SOURCE: &str = r#"
state model Task {
    state title = ""
}

state model Board {
    state tasks: [live Task] = []
    state trash: [live Task] = []
}

state left: Board
state right: Board
state selected: maybe live Task = none
state recent: maybe live Task = none

action seed {
    create Task in left as task {
        through task.title = "Draft"
        insert task into left.tasks
        insert task into right.tasks
        insert task into right.trash
        insert task into right.trash
    }
    selected = left.tasks[0]
    recent = left.tasks[0]
}

action permanentDelete {
    destroy selected in left
}

action destroyThenFail {
    destroy selected in left
    fail "later"
}

action foreignDestroy {
    destroy selected in right
}

action destroyAbsent {
    destroy selected in left
}

action createThenDestroy {
    create Task in left as task {
        insert task into left.tasks
    }
    selected = left.tasks[0]
    destroy selected in left
}
"#;

fn runtime(source: &str) -> Runtime {
    let checked = check_source_with_runtime_models(source).expect("source should check");
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
fn owner_relative_destroy_ends_committed_leaf_lifetime_and_cleans_nonowning_state() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("seed").expect("seed should commit");

    let identity = targets(&mut runtime, "__meld_mseq$left$tasks")[0].clone();
    assert_eq!(
        targets(&mut runtime, "__meld_mseq$right$trash"),
        vec![identity.clone(), identity.clone()]
    );

    runtime
        .run_action("permanentDelete")
        .expect("root owner should be able to destroy committed leaf child");

    assert!(targets(&mut runtime, "__meld_mseq$left$tasks").is_empty());
    assert!(targets(&mut runtime, "__meld_mseq$right$tasks").is_empty());
    assert!(targets(&mut runtime, "__meld_mseq$right$trash").is_empty());
    assert_eq!(
        runtime.value("__meld_live$selected").unwrap(),
        Value::String(String::new())
    );
    assert_eq!(
        runtime.value("__meld_live$recent").unwrap(),
        Value::String(String::new())
    );
}

#[test]
fn destroy_rolls_back_with_later_action_failure() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("seed").expect("seed should commit");
    let identity = targets(&mut runtime, "__meld_mseq$left$tasks")[0].clone();

    let error = runtime
        .run_action("destroyThenFail")
        .expect_err("later failure should roll back destruction");
    assert!(error.message.contains("later"));

    assert_eq!(
        targets(&mut runtime, "__meld_mseq$left$tasks"),
        vec![identity.clone()]
    );
    assert_eq!(
        targets(&mut runtime, "__meld_mseq$right$trash"),
        vec![identity.clone(), identity.clone()]
    );
    assert_eq!(
        runtime.value("__meld_live$selected").unwrap(),
        Value::String(identity)
    );
}

#[test]
fn foreign_membership_owner_cannot_destroy_child() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("seed").expect("seed should commit");
    let identity = targets(&mut runtime, "__meld_mseq$left$tasks")[0].clone();

    let error = runtime
        .run_action("foreignDestroy")
        .expect_err("foreign membership owner must not gain lifetime authority");
    assert!(error.message.contains("rooting owner 'left'"));
    assert!(error.message.contains("not 'right'"));

    assert_eq!(
        targets(&mut runtime, "__meld_mseq$left$tasks"),
        vec![identity.clone()]
    );
    assert_eq!(
        runtime.value("__meld_live$selected").unwrap(),
        Value::String(identity)
    );
}

#[test]
fn absent_maybe_live_designation_cannot_destroy() {
    let mut runtime = runtime(SOURCE);
    let error = runtime
        .run_action("destroyAbsent")
        .expect_err("absent designation must not name a child to destroy");
    assert!(error.message.contains("present maybe live designation"));
}

#[test]
fn transaction_local_fresh_child_cannot_use_committed_child_destroy_surface() {
    let mut runtime = runtime(SOURCE);
    let error = runtime
        .run_action("createThenDestroy")
        .expect_err("first destroy surface must not select fresh-child cancellation semantics");
    assert!(error.message.contains("existing committed dynamic child"));

    assert!(targets(&mut runtime, "__meld_mseq$left$tasks").is_empty());
    assert_eq!(
        runtime.value("__meld_live$selected").unwrap(),
        Value::String(String::new())
    );
}

#[test]
fn destroy_rejects_plain_live_designation_at_compile_time() {
    let source = r#"
state model Task { state title = "" }
state model Board { state tasks: [live Task] = [] }
state fallback: Task
state board: Board
state selected: live Task = live fallback

action attempt {
    destroy selected in board
}
"#;

    let errors = check_source_with_runtime_models(source).expect_err("plain live must be rejected");
    assert!(errors
        .iter()
        .any(|error| error.message.contains("plain live Task")
            && error.message.contains("cannot become absent")));
}

#[test]
fn destroy_rejects_unknown_owner_at_compile_time() {
    let source = r#"
state model Task { state title = "" }
state selected: maybe live Task = none

action attempt {
    destroy selected in nowhere
}
"#;

    let errors =
        check_source_with_runtime_models(source).expect_err("unknown owner must be rejected");
    assert!(errors
        .iter()
        .any(|error| error.message.contains("destroy owner 'nowhere'")
            && error.message.contains("not a modeled-state root")));
}
