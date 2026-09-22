use super::{Runtime, RuntimeError, StateCell, Transaction, Value};
use crate::ast::Expr;
use crate::relative_navigation::{
    resolve_unique_neighbor, RelativeDirection, RelativeNeighborError,
};
use crate::semantic::ValueType;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, PartialEq)]
struct RelativeSelectionSpec {
    designation: Expr,
    source: Expr,
    direction: RelativeDirection,
    element_model: String,
}

fn sequence(model: &str, targets: &[&str]) -> Value {
    Value::Sequence {
        element_model: model.to_string(),
        targets: targets.iter().map(|target| (*target).to_string()).collect(),
    }
}

fn state(value: Value, value_type: ValueType) -> StateCell {
    StateCell {
        value,
        value_type,
        dependents: HashSet::new(),
    }
}

fn runtime_with_selection(selected: &str, source_targets: &[&str]) -> Runtime {
    let mut states = HashMap::new();
    states.insert(
        "selected".to_string(),
        state(Value::String(selected.to_string()), ValueType::String),
    );
    states.insert(
        "sequence".to_string(),
        state(
            sequence("Task", source_targets),
            ValueType::SequenceLive("Task".to_string()),
        ),
    );
    states.insert(
        "view".to_string(),
        state(
            sequence("Task", source_targets),
            ValueType::SequenceLive("Task".to_string()),
        ),
    );
    states.insert("marker".to_string(), state(Value::Int(0), ValueType::Int));

    Runtime {
        states,
        derived: HashMap::new(),
        actions: HashMap::new(),
        action_parameter_types: HashMap::new(),
        binding_order: Vec::new(),
        eval_stack: Vec::new(),
        action_stack: Vec::new(),
        action_frames: Vec::new(),
        reductions: HashMap::new(),
        reduction_frames: Vec::new(),
        element_frames: Vec::new(),
        model_frames: Vec::new(),
        runtime_model_templates: HashMap::new(),
        runtime_model_roots: HashMap::new(),
        runtime_designations: HashMap::new(),
        dynamic_model_owners: HashMap::new(),
        runtime_index_grant_carriers: HashMap::new(),
        next_dynamic_identity: 0,
        transaction: None,
    }
}

fn spec(source: &str, direction: RelativeDirection) -> RelativeSelectionSpec {
    RelativeSelectionSpec {
        designation: Expr::Name("selected".to_string()),
        source: Expr::Name(source.to_string()),
        direction,
        element_model: "Task".to_string(),
    }
}

fn resolve_relative_selection(
    runtime: &mut Runtime,
    selection: &RelativeSelectionSpec,
) -> Result<Value, RuntimeError> {
    let designation = match runtime.eval_expr(&selection.designation, None)? {
        Value::String(identity) if identity.is_empty() => {
            return Err(RuntimeError::new(
                "relative selection requires a present live designation",
            ));
        }
        Value::String(identity) => identity,
        other => {
            return Err(RuntimeError::new(format!(
                "relative selection expected a live designation, found {other}"
            )));
        }
    };

    let source = runtime.eval_expr(&selection.source, None)?;
    let Value::Sequence {
        element_model,
        targets,
    } = source
    else {
        return Err(RuntimeError::new(
            "relative selection source is not an ordered live sequence/view",
        ));
    };

    if element_model != selection.element_model {
        return Err(RuntimeError::new(format!(
            "relative selection expected live {} structure, found live {} structure",
            selection.element_model, element_model
        )));
    }

    let neighbor =
        resolve_unique_neighbor(&designation, &targets, selection.direction).map_err(|error| {
            match error {
                RelativeNeighborError::NoCurrentOccurrence => RuntimeError::new(format!(
                    "relative selection anchor '{designation}' has no current occurrence"
                )),
                RelativeNeighborError::AmbiguousCurrentOccurrence => RuntimeError::new(format!(
                    "relative selection anchor '{designation}' has multiple current occurrences"
                )),
                RelativeNeighborError::Boundary => {
                    RuntimeError::new("relative selection has no neighbor in that direction")
                }
            }
        })?;

    Ok(Value::String(neighbor))
}

fn apply_relative_selection(
    runtime: &mut Runtime,
    target_state: &str,
    selection: &RelativeSelectionSpec,
) -> Result<(), RuntimeError> {
    let neighbor = resolve_relative_selection(runtime, selection)?;
    runtime.write_state(target_state, neighbor)
}

fn run_test_transaction(
    runtime: &mut Runtime,
    operation: impl FnOnce(&mut Runtime) -> Result<(), RuntimeError>,
) -> Result<(), RuntimeError> {
    assert!(
        runtime.transaction.is_none(),
        "test transaction must start outside another transaction"
    );
    runtime.transaction = Some(Transaction::default());

    match operation(runtime) {
        Ok(()) => {
            let transaction = runtime
                .transaction
                .take()
                .expect("test transaction should exist");
            runtime.commit(transaction);
            Ok(())
        }
        Err(error) => {
            runtime.transaction = None;
            Err(error)
        }
    }
}

#[test]
fn relative_selection_uses_transaction_visible_sequence_and_returns_child_identity() {
    let mut runtime = runtime_with_selection("B", &["A", "B", "C"]);

    run_test_transaction(&mut runtime, |runtime| {
        runtime.write_state("sequence", sequence("Task", &["X", "A", "B", "C"]))?;
        apply_relative_selection(
            runtime,
            "selected",
            &spec("sequence", RelativeDirection::Next),
        )
    })
    .unwrap();

    assert_eq!(
        runtime.value("selected").unwrap(),
        Value::String("C".to_string())
    );
}

#[test]
fn relative_selection_uses_transaction_visible_reselected_anchor() {
    let mut runtime = runtime_with_selection("B", &["A", "B", "C", "D"]);

    run_test_transaction(&mut runtime, |runtime| {
        runtime.write_state("selected", Value::String("C".to_string()))?;
        apply_relative_selection(
            runtime,
            "selected",
            &spec("sequence", RelativeDirection::Next),
        )
    })
    .unwrap();

    assert_eq!(
        runtime.value("selected").unwrap(),
        Value::String("D".to_string())
    );
}

#[test]
fn relative_selection_consumes_current_view_value_after_view_change() {
    let mut runtime = runtime_with_selection("B", &["A", "B", "D"]);

    run_test_transaction(&mut runtime, |runtime| {
        runtime.write_state("view", sequence("Task", &["A", "B", "C", "D"]))?;
        apply_relative_selection(runtime, "selected", &spec("view", RelativeDirection::Next))
    })
    .unwrap();

    assert_eq!(
        runtime.value("selected").unwrap(),
        Value::String("C".to_string())
    );
}

#[test]
fn previous_and_next_consume_only_transient_anchor_location() {
    let mut next_runtime = runtime_with_selection("B", &["A", "B", "C"]);
    run_test_transaction(&mut next_runtime, |runtime| {
        apply_relative_selection(
            runtime,
            "selected",
            &spec("sequence", RelativeDirection::Next),
        )
    })
    .unwrap();
    assert_eq!(
        next_runtime.value("selected").unwrap(),
        Value::String("C".to_string())
    );

    let mut previous_runtime = runtime_with_selection("B", &["A", "B", "C"]);
    run_test_transaction(&mut previous_runtime, |runtime| {
        apply_relative_selection(
            runtime,
            "selected",
            &spec("sequence", RelativeDirection::Previous),
        )
    })
    .unwrap();
    assert_eq!(
        previous_runtime.value("selected").unwrap(),
        Value::String("A".to_string())
    );
}

fn assert_failure_rolls_back(
    selected: &str,
    targets: &[&str],
    direction: RelativeDirection,
    expected_message: &str,
) {
    let mut runtime = runtime_with_selection(selected, targets);

    let error = run_test_transaction(&mut runtime, |runtime| {
        runtime.write_state("marker", Value::Int(1))?;
        apply_relative_selection(runtime, "selected", &spec("sequence", direction))
    })
    .expect_err("relative selection should fail");

    assert!(
        error.message.contains(expected_message),
        "{}",
        error.message
    );
    assert_eq!(runtime.value("marker").unwrap(), Value::Int(0));
    assert_eq!(
        runtime.value("selected").unwrap(),
        Value::String(selected.to_string())
    );
}

#[test]
fn relative_selection_failures_roll_back_prior_staged_writes() {
    assert_failure_rolls_back(
        "",
        &["A", "B", "C"],
        RelativeDirection::Next,
        "present live designation",
    );
    assert_failure_rolls_back(
        "D",
        &["A", "B", "C"],
        RelativeDirection::Next,
        "no current occurrence",
    );
    assert_failure_rolls_back(
        "B",
        &["A", "B", "C", "B"],
        RelativeDirection::Next,
        "multiple current occurrences",
    );
    assert_failure_rolls_back(
        "A",
        &["A", "B", "C"],
        RelativeDirection::Previous,
        "no neighbor",
    );
    assert_failure_rolls_back(
        "C",
        &["A", "B", "C"],
        RelativeDirection::Next,
        "no neighbor",
    );
}

#[test]
fn later_failure_rolls_back_successful_relative_reselection() {
    let mut runtime = runtime_with_selection("B", &["A", "B", "C"]);

    let error = run_test_transaction(&mut runtime, |runtime| {
        apply_relative_selection(
            runtime,
            "selected",
            &spec("sequence", RelativeDirection::Next),
        )?;
        assert_eq!(
            runtime.read_name("selected", None)?,
            Value::String("C".to_string())
        );
        Err(RuntimeError::new("later failure"))
    })
    .expect_err("later failure should abort the transaction");

    assert_eq!(error.message, "later failure");
    assert_eq!(
        runtime.value("selected").unwrap(),
        Value::String("B".to_string())
    );
}

#[test]
fn wrong_model_structure_is_rejected_without_exposing_position() {
    let mut runtime = runtime_with_selection("B", &["A", "B", "C"]);
    runtime.states.insert(
        "sequence".to_string(),
        state(
            sequence("Other", &["A", "B", "C"]),
            ValueType::SequenceLive("Other".to_string()),
        ),
    );

    let error = run_test_transaction(&mut runtime, |runtime| {
        resolve_relative_selection(runtime, &spec("sequence", RelativeDirection::Next)).map(|_| ())
    })
    .expect_err("wrong-model source should fail");

    assert!(error.message.contains("expected live Task structure"));
    assert!(!error.message.contains("index"));
    assert!(!error.message.contains("__meld"));
}
