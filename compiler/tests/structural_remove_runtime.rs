use elanu_compiler::{
    check_source_with_runtime_models,
    runtime::{Runtime, Value},
};

fn runtime(source: &str) -> Runtime {
    let checked = check_source_with_runtime_models(source).expect("source should check");
    Runtime::from_checked_source(&checked).expect("runtime should initialize")
}

const SOURCE: &str = r#"
state model LineItem {
    state quantity = 1
}

state model Invoice {
    state lines: [live LineItem] = []
    derived total = reduce lines from 0 as (sum, line) {
        sum + line.quantity
    }
}

state invoice: Invoice
state selectedLine = 0
state observed = 0

action setup {
    create LineItem in invoice as first {
        through first.quantity = 2
        insert first into invoice.lines
        insert first into invoice.lines
    }
    create LineItem in invoice as second {
        through second.quantity = 5
        insert second into invoice.lines
    }
    observed = invoice.total
}

action removeFirst {
    remove invoice.lines[0]
    observed = invoice.total
}

action removeFirstTwice {
    remove invoice.lines[0]
    remove invoice.lines[0]
    observed = invoice.total
}

action removeThenFail {
    remove invoice.lines[0]
    observed = invoice.total
    fail "rollback removal"
}

action removeMissing {
    remove invoice.lines[9]
}

action removeSelected {
    remove invoice.lines[selectedLine]
    observed = invoice.total
}

action selectSecondThenRemove {
    selectedLine = 1
    remove invoice.lines[selectedLine]
    observed = invoice.total
}

action removeSelectedExpression {
    remove invoice.lines[selectedLine + 1]
    observed = invoice.total
}

action removeFirstThenReadShiftedStructure {
    selectedLine = 0
    remove invoice.lines[selectedLine]
    observed = invoice.lines[1].quantity
}

action selectNegativeThenRemove {
    selectedLine = 0 - 1
    remove invoice.lines[selectedLine]
}

action selectMissingThenRemove {
    selectedLine = 9
    remove invoice.lines[selectedLine]
}

action selectSecondRemoveThenFail {
    selectedLine = 1
    remove invoice.lines[selectedLine]
    observed = invoice.total
    fail "rollback selected removal"
}
"#;

fn sequence(runtime: &mut Runtime) -> (String, Vec<String>) {
    let Value::Sequence {
        element_model,
        targets,
    } = runtime.value("__elanu_mseq$invoice$lines").unwrap()
    else {
        panic!("invoice.lines should be realized as a runtime sequence");
    };
    (element_model, targets)
}

#[test]
fn removal_targets_one_structural_occurrence_not_child_identity() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("setup").expect("setup should commit");

    let (model, before) = sequence(&mut runtime);
    assert_eq!(model, "LineItem");
    assert_eq!(before.len(), 3);
    assert_eq!(before[0], before[1]);
    assert_ne!(before[0], before[2]);
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(9));

    runtime
        .run_action("removeFirst")
        .expect("removal should commit");

    let (_, after) = sequence(&mut runtime);
    assert_eq!(after.len(), 2);
    assert_eq!(after[0], before[0]);
    assert_eq!(after[1], before[2]);
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(7));
}

#[test]
fn repeated_position_removal_re_evaluates_current_structure() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("setup").expect("setup should commit");
    let (_, before) = sequence(&mut runtime);

    runtime
        .run_action("removeFirstTwice")
        .expect("both removals should commit");

    let (_, after) = sequence(&mut runtime);
    assert_eq!(after, vec![before[2].clone()]);
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(5));
}

#[test]
fn failed_removal_rolls_back_membership_and_staged_observation() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("setup").expect("setup should commit");
    let (_, before) = sequence(&mut runtime);

    let error = runtime
        .run_action("removeThenFail")
        .expect_err("failure should roll back removal");

    assert!(error.message.contains("rollback removal"));
    let (_, after) = sequence(&mut runtime);
    assert_eq!(after, before);
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(9));
}

#[test]
fn out_of_bounds_removal_fails_without_changing_membership() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("setup").expect("setup should commit");
    let (_, before) = sequence(&mut runtime);

    let error = runtime
        .run_action("removeMissing")
        .expect_err("out-of-bounds removal should fail");

    assert!(error.message.contains("out of bounds"));
    let (_, after) = sequence(&mut runtime);
    assert_eq!(after, before);
}

#[test]
fn removal_itself_forces_runtime_sequence_realization() {
    let mut runtime = runtime(
        r#"
state model LineItem {
    state quantity = 1
}

state model Invoice {
    state lines: [live LineItem] = []
}

state item: LineItem
state invoice: Invoice

action seed {
    invoice.lines = [live item]
}

action removeOnly {
    remove invoice.lines[0]
}
"#,
    );

    runtime.run_action("seed").expect("seed should commit");
    let (_, before) = sequence(&mut runtime);
    assert_eq!(before.len(), 1);

    runtime
        .run_action("removeOnly")
        .expect("removal should commit");
    let (_, after) = sequence(&mut runtime);
    assert!(after.is_empty());
}

#[test]
fn runtime_selected_removal_is_zero_based_and_removes_only_one_occurrence() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("setup").expect("setup should commit");
    let (_, before) = sequence(&mut runtime);

    runtime
        .run_action("removeSelected")
        .expect("selected position zero should remove the first occurrence");

    let (_, after) = sequence(&mut runtime);
    assert_eq!(after, vec![before[1].clone(), before[2].clone()]);
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(7));
}

#[test]
fn staged_selector_state_selects_the_current_occurrence() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("setup").expect("setup should commit");
    let (_, before) = sequence(&mut runtime);

    runtime
        .run_action("selectSecondThenRemove")
        .expect("removal should observe the staged selector value");

    let (_, after) = sequence(&mut runtime);
    assert_eq!(after, vec![before[0].clone(), before[2].clone()]);
    assert_eq!(runtime.value("selectedLine").unwrap(), Value::Int(1));
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(7));
}

#[test]
fn arbitrary_current_int_expression_can_select_removal_position() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("setup").expect("setup should commit");
    let (_, before) = sequence(&mut runtime);

    runtime
        .run_action("removeSelectedExpression")
        .expect("ordinary Int expression should select the removal position");

    let (_, after) = sequence(&mut runtime);
    assert_eq!(after, vec![before[0].clone(), before[2].clone()]);
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(7));
}

#[test]
fn later_operations_in_the_same_action_observe_updated_structure() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("setup").expect("setup should commit");

    runtime
        .run_action("removeFirstThenReadShiftedStructure")
        .expect("later indexed read should see post-removal structure");

    assert_eq!(runtime.value("observed").unwrap(), Value::Int(5));
}

#[test]
fn negative_runtime_selected_removal_fails_and_rolls_back_selector() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("setup").expect("setup should commit");
    let (_, before) = sequence(&mut runtime);

    let error = runtime
        .run_action("selectNegativeThenRemove")
        .expect_err("negative removal position should fail at runtime");

    assert!(error.message.contains("cannot be negative"));
    assert_eq!(runtime.value("selectedLine").unwrap(), Value::Int(0));
    let (_, after) = sequence(&mut runtime);
    assert_eq!(after, before);
}

#[test]
fn out_of_bounds_runtime_selected_removal_rolls_back_selector_and_membership() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("setup").expect("setup should commit");
    let (_, before) = sequence(&mut runtime);

    let error = runtime
        .run_action("selectMissingThenRemove")
        .expect_err("out-of-bounds selected removal should fail");

    assert!(error.message.contains("out of bounds"));
    assert_eq!(runtime.value("selectedLine").unwrap(), Value::Int(0));
    let (_, after) = sequence(&mut runtime);
    assert_eq!(after, before);
}

#[test]
fn later_failure_restores_runtime_selected_membership_and_selector_state() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("setup").expect("setup should commit");
    let (_, before) = sequence(&mut runtime);

    let error = runtime
        .run_action("selectSecondRemoveThenFail")
        .expect_err("later failure should roll back selected removal");

    assert!(error.message.contains("rollback selected removal"));
    assert_eq!(runtime.value("selectedLine").unwrap(), Value::Int(0));
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(9));
    let (_, after) = sequence(&mut runtime);
    assert_eq!(after, before);
}

#[test]
fn removing_all_occurrences_does_not_destroy_the_removed_child_state() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("setup").expect("setup should commit");
    let (_, before) = sequence(&mut runtime);
    let first_child = before[0].clone();

    runtime
        .run_action("removeFirstTwice")
        .expect("both first-child occurrences should be removable");

    let (_, after) = sequence(&mut runtime);
    assert_eq!(after, vec![before[2].clone()]);
    assert_eq!(
        runtime
            .value(&format!("__elanu_sm${first_child}$quantity"))
            .unwrap(),
        Value::Int(2)
    );
}

#[test]
fn runtime_selected_removal_index_must_be_int() {
    let errors = check_source_with_runtime_models(
        r#"
state model LineItem {
    state quantity = 1
}
state model Invoice {
    state lines: [live LineItem] = []
}
state invoice: Invoice
state selectedLine = 0.5

action removeSelected {
    remove invoice.lines[selectedLine]
}
"#,
    )
    .expect_err("Float removal selector should be rejected statically");

    assert!(errors.iter().any(|error| {
        error
            .message
            .contains("argument for value parameter 'index' has type Float but expected Int")
    }));
    assert!(errors
        .iter()
        .all(|error| !error.message.contains("__elanu_remove_occurrence_")));
}
