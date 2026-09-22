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
    state quantity = 0
    state active = true
}

state model Invoice {
    state selected = 0
    state lines: [live LineItem] = []

    derived activeLines = filter lines as line {
        line.active
    }
}

state a: LineItem
state b: LineItem
state c: LineItem
state invoice: Invoice
state seen = 0

derived observedSelected = invoice.selected

action setupDistinct {
    a.quantity = 1
    b.quantity = 2
    c.quantity = 3
    invoice.lines = [live a, live b, live c]
}

action setupWithHiddenMiddle {
    a.quantity = 1
    b.quantity = 2
    b.active = false
    c.quantity = 3
    invoice.lines = [live a, live b, live c]
}

action setupDuplicate {
    a.quantity = 1
    b.quantity = 2
    invoice.lines = [live a, live b, live a]
}

action removeMiddleVisible {
    invoice.selected = 1
    remove invoice.activeLines[invoice.selected]
    seen = invoice.lines[1].quantity
}

action removeSecondVisibleAcrossHiddenSourceOccurrence {
    invoice.selected = 1
    remove invoice.activeLines[invoice.selected]
    seen = invoice.lines[1].quantity
}

action removeThirdDuplicate {
    invoice.selected = 2
    remove invoice.activeLines[invoice.selected]
    seen = invoice.lines[0].quantity
}

action predicateThenRemove {
    b.active = false
    invoice.selected = 1
    b.active = true
    remove invoice.activeLines[invoice.selected]
    seen = invoice.lines[1].quantity
}

action membershipThenRemove {
    invoice.lines = [live c, live a, live b]
    invoice.selected = 1
    remove invoice.activeLines[invoice.selected]
    seen = invoice.lines[1].quantity
}

action removeFirstTwice {
    invoice.selected = 0
    remove invoice.activeLines[invoice.selected]
    remove invoice.activeLines[invoice.selected]
    seen = invoice.lines[0].quantity
}

action invalidRemoval {
    invoice.selected = 9
    remove invoice.activeLines[invoice.selected]
}

action negativeRemoval {
    invoice.selected = 0 - 1
    remove invoice.activeLines[invoice.selected]
}

action removeThenFail {
    invoice.selected = 1
    remove invoice.activeLines[invoice.selected]
    fail "rollback removal"
}

action observeMiddle {
    seen = invoice.lines[1].quantity
}
"#;

#[test]
fn filtered_removal_maps_selected_view_occurrence_to_backing_membership() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("setupDistinct").unwrap();
    runtime.run_action("removeMiddleVisible").unwrap();

    assert_eq!(runtime.value("seen").unwrap(), Value::Int(3));
}

#[test]
fn filtered_removal_skips_nonretained_backing_occurrences() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("setupWithHiddenMiddle").unwrap();
    runtime
        .run_action("removeSecondVisibleAcrossHiddenSourceOccurrence")
        .unwrap();

    // Source is [a, b, c] while current view is [a, c]. Removing view position 1
    // must remove backing position 2 (c), leaving b at source position 1.
    assert_eq!(runtime.value("seen").unwrap(), Value::Int(2));
}

#[test]
fn duplicate_filtered_occurrences_preserve_backing_occurrence_position() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("setupDuplicate").unwrap();
    runtime.run_action("removeThirdDuplicate").unwrap();

    // Removing the third view occurrence from [a, b, a] must leave [a, b].
    // Resolving only child identity would be insufficient because both selected
    // and first occurrences designate the same child.
    assert_eq!(runtime.value("seen").unwrap(), Value::Int(1));
}

#[test]
fn staged_predicate_change_controls_filtered_removal_mapping() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("setupDistinct").unwrap();
    runtime.run_action("predicateThenRemove").unwrap();

    // b is visible again before removal, so selected view position 1 removes b
    // and c becomes source position 1.
    assert_eq!(runtime.value("seen").unwrap(), Value::Int(3));
}

#[test]
fn staged_membership_change_controls_filtered_removal_mapping() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("setupDistinct").unwrap();
    runtime.run_action("membershipThenRemove").unwrap();

    // Current source/view is [c, a, b]; removing view position 1 removes a.
    assert_eq!(runtime.value("seen").unwrap(), Value::Int(2));
}

#[test]
fn repeated_filtered_removal_re_evaluates_the_current_view() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("setupDistinct").unwrap();
    runtime.run_action("removeFirstTwice").unwrap();

    assert_eq!(runtime.value("seen").unwrap(), Value::Int(3));
}

#[test]
fn invalid_filtered_removal_rolls_back_selector_and_membership() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("setupDistinct").unwrap();

    assert!(runtime.run_action("invalidRemoval").is_err());
    assert_eq!(runtime.value("observedSelected").unwrap(), Value::Int(0));

    runtime.run_action("observeMiddle").unwrap();
    assert_eq!(runtime.value("seen").unwrap(), Value::Int(2));
}

#[test]
fn negative_filtered_removal_rolls_back_selector_and_membership() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("setupDistinct").unwrap();

    assert!(runtime.run_action("negativeRemoval").is_err());
    assert_eq!(runtime.value("observedSelected").unwrap(), Value::Int(0));

    runtime.run_action("observeMiddle").unwrap();
    assert_eq!(runtime.value("seen").unwrap(), Value::Int(2));
}

#[test]
fn later_failure_rolls_back_filtered_structural_removal() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("setupDistinct").unwrap();

    assert!(runtime.run_action("removeThenFail").is_err());
    runtime.run_action("observeMiddle").unwrap();
    assert_eq!(runtime.value("seen").unwrap(), Value::Int(2));
}
