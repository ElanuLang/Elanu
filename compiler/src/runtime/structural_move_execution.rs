use super::{Runtime, RuntimeError, StateCell, Transaction, Value};
use crate::ast::Expr;
use crate::semantic::ValueType;
use crate::structural_move::{move_unique_relative, RelativePlacement, StructuralMoveError};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone)]
struct MoveSpec {
    backing: String,
    selection: String,
    moving: Expr,
    anchor: Expr,
    placement: RelativePlacement,
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
        designation: None,
        dependents: HashSet::new(),
    }
}

fn runtime_with_structure(backing: &[&str], view: &[&str]) -> Runtime {
    let mut states = HashMap::new();
    states.insert(
        "sequence".to_string(),
        state(
            sequence("Task", backing),
            ValueType::SequenceLive("Task".to_string()),
        ),
    );
    states.insert(
        "view".to_string(),
        state(
            sequence("Task", view),
            ValueType::SequenceLive("Task".to_string()),
        ),
    );
    states.insert(
        "moving".to_string(),
        state(Value::String("B".to_string()), ValueType::String),
    );
    states.insert(
        "anchor".to_string(),
        state(Value::String("C".to_string()), ValueType::String),
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
        dynamic_model_types: HashMap::new(),
        runtime_index_grant_carriers: HashMap::new(),
        next_dynamic_identity: 0,
        transaction: None,
    }
}

fn spec(selection: &str, placement: RelativePlacement) -> MoveSpec {
    MoveSpec {
        backing: "sequence".to_string(),
        selection: selection.to_string(),
        moving: Expr::Name("moving".to_string()),
        anchor: Expr::Name("anchor".to_string()),
        placement,
        element_model: "Task".to_string(),
    }
}

fn designation(
    runtime: &mut Runtime,
    expression: &Expr,
    role: &str,
) -> Result<String, RuntimeError> {
    match runtime.eval_expr(expression, None)? {
        Value::String(identity) if identity.is_empty() => Err(RuntimeError::new(format!(
            "structural movement {role} requires a present live designation"
        ))),
        Value::String(identity) => Ok(identity),
        other => Err(RuntimeError::new(format!(
            "structural movement {role} expected a live designation, found {other}"
        ))),
    }
}

fn ordered_targets(
    runtime: &mut Runtime,
    name: &str,
    expected_model: &str,
    role: &str,
) -> Result<Vec<String>, RuntimeError> {
    let value = runtime.read_name(name, None)?;
    let Value::Sequence {
        element_model,
        targets,
    } = value
    else {
        return Err(RuntimeError::new(format!(
            "structural movement {role} is not ordered live structure"
        )));
    };
    if element_model != expected_model {
        return Err(RuntimeError::new(format!(
            "structural movement expected live {expected_model} {role}, found live {element_model} {role}"
        )));
    }
    Ok(targets)
}

fn apply_move(runtime: &mut Runtime, movement: &MoveSpec) -> Result<(), RuntimeError> {
    let moving = designation(runtime, &movement.moving, "moving child")?;
    let anchor = designation(runtime, &movement.anchor, "anchor child")?;
    let backing = ordered_targets(
        runtime,
        &movement.backing,
        &movement.element_model,
        "backing membership",
    )?;
    let selection = ordered_targets(
        runtime,
        &movement.selection,
        &movement.element_model,
        "selection structure",
    )?;

    let reordered =
        move_unique_relative(&backing, &selection, &moving, &anchor, movement.placement).map_err(
            |error| {
                let message = match error {
                    StructuralMoveError::MovingNoCurrentOccurrence => {
                        format!("moving child '{moving}' has no current occurrence")
                    }
                    StructuralMoveError::MovingAmbiguousCurrentOccurrence => {
                        format!("moving child '{moving}' has multiple current occurrences")
                    }
                    StructuralMoveError::AnchorNoCurrentOccurrence => {
                        format!("anchor child '{anchor}' has no current occurrence")
                    }
                    StructuralMoveError::AnchorAmbiguousCurrentOccurrence => {
                        format!("anchor child '{anchor}' has multiple current occurrences")
                    }
                    StructuralMoveError::ViewDoesNotMapToBacking => {
                        "selection structure does not map to current backing membership".to_string()
                    }
                };
                RuntimeError::new(message)
            },
        )?;

    runtime.write_state(
        &movement.backing,
        Value::Sequence {
            element_model: movement.element_model.clone(),
            targets: reordered,
        },
    )
}

fn run_test_transaction(
    runtime: &mut Runtime,
    operation: impl FnOnce(&mut Runtime) -> Result<(), RuntimeError>,
) -> Result<(), RuntimeError> {
    assert!(runtime.transaction.is_none());
    runtime.transaction = Some(Transaction::default());
    match operation(runtime) {
        Ok(()) => {
            let transaction = runtime.transaction.take().unwrap();
            runtime.commit(transaction);
            Ok(())
        }
        Err(error) => {
            runtime.transaction = None;
            Err(error)
        }
    }
}

fn targets(runtime: &mut Runtime, name: &str) -> Vec<String> {
    match runtime.value(name).unwrap() {
        Value::Sequence { targets, .. } => targets,
        other => panic!("expected sequence, found {other}"),
    }
}

#[test]
fn direct_movement_commits_new_order_without_changing_child_identity() {
    let mut runtime = runtime_with_structure(&["A", "B", "C", "D"], &["A", "B", "C", "D"]);
    runtime
        .write_state("moving", Value::String("B".to_string()))
        .expect_err("writes outside actions remain forbidden");

    run_test_transaction(&mut runtime, |runtime| {
        runtime.write_state("moving", Value::String("B".to_string()))?;
        runtime.write_state("anchor", Value::String("D".to_string()))?;
        apply_move(runtime, &spec("sequence", RelativePlacement::After))
    })
    .unwrap();

    assert_eq!(targets(&mut runtime, "sequence"), vec!["A", "C", "D", "B"]);
}

#[test]
fn movement_observes_transaction_visible_membership_and_designations() {
    let mut runtime = runtime_with_structure(&["A", "B", "C", "D"], &["A", "B", "C", "D"]);

    run_test_transaction(&mut runtime, |runtime| {
        runtime.write_state("sequence", sequence("Task", &["X", "A", "B", "C", "D"]))?;
        runtime.write_state("moving", Value::String("C".to_string()))?;
        runtime.write_state("anchor", Value::String("A".to_string()))?;
        apply_move(runtime, &spec("sequence", RelativePlacement::Before))?;
        assert_eq!(
            ordered_targets(runtime, "sequence", "Task", "backing")?,
            vec!["X", "C", "A", "B", "D"]
        );
        Ok(())
    })
    .unwrap();
}

#[test]
fn filtered_view_movement_updates_backing_and_preserves_hidden_rows() {
    let mut runtime = runtime_with_structure(
        &["A", "hidden1", "B", "hidden2", "C", "D"],
        &["A", "B", "C", "D"],
    );

    run_test_transaction(&mut runtime, |runtime| {
        runtime.write_state("moving", Value::String("D".to_string()))?;
        runtime.write_state("anchor", Value::String("B".to_string()))?;
        apply_move(runtime, &spec("view", RelativePlacement::Before))
    })
    .unwrap();

    assert_eq!(
        targets(&mut runtime, "sequence"),
        vec!["A", "hidden1", "D", "B", "hidden2", "C"]
    );
}

#[test]
fn transaction_visible_view_controls_filtered_mapping() {
    let mut runtime = runtime_with_structure(&["A", "B", "C", "D"], &["A", "B", "D"]);

    run_test_transaction(&mut runtime, |runtime| {
        runtime.write_state("view", sequence("Task", &["A", "B", "C", "D"]))?;
        runtime.write_state("moving", Value::String("C".to_string()))?;
        runtime.write_state("anchor", Value::String("D".to_string()))?;
        apply_move(runtime, &spec("view", RelativePlacement::After))
    })
    .unwrap();

    assert_eq!(targets(&mut runtime, "sequence"), vec!["A", "B", "D", "C"]);
}

#[test]
fn later_failure_rolls_back_successful_movement() {
    let mut runtime = runtime_with_structure(&["A", "B", "C"], &["A", "B", "C"]);

    let error = run_test_transaction(&mut runtime, |runtime| {
        runtime.write_state("moving", Value::String("B".to_string()))?;
        runtime.write_state("anchor", Value::String("C".to_string()))?;
        apply_move(runtime, &spec("sequence", RelativePlacement::After))?;
        runtime.write_state("marker", Value::Int(1))?;
        assert_eq!(
            ordered_targets(runtime, "sequence", "Task", "backing")?,
            vec!["A", "C", "B"]
        );
        Err(RuntimeError::new("later failure"))
    })
    .expect_err("later failure must abort structural movement");

    assert_eq!(error.message, "later failure");
    assert_eq!(targets(&mut runtime, "sequence"), vec!["A", "B", "C"]);
    assert_eq!(runtime.value("marker").unwrap(), Value::Int(0));
}

#[test]
fn ambiguity_and_absence_fail_without_committing_prior_writes() {
    for (backing, moving, anchor, expected) in [
        (
            vec!["A", "B", "B", "C"],
            "B",
            "C",
            "moving child 'B' has multiple current occurrences",
        ),
        (
            vec!["A", "B", "C", "C"],
            "B",
            "C",
            "anchor child 'C' has multiple current occurrences",
        ),
        (
            vec!["A", "B", "C"],
            "D",
            "C",
            "moving child 'D' has no current occurrence",
        ),
    ] {
        let refs = backing.iter().copied().collect::<Vec<_>>();
        let mut runtime = runtime_with_structure(&refs, &refs);
        let error = run_test_transaction(&mut runtime, |runtime| {
            runtime.write_state("marker", Value::Int(1))?;
            runtime.write_state("moving", Value::String(moving.to_string()))?;
            runtime.write_state("anchor", Value::String(anchor.to_string()))?;
            apply_move(runtime, &spec("sequence", RelativePlacement::After))
        })
        .expect_err("invalid movement should fail");
        assert!(error.message.contains(expected), "{}", error.message);
        assert_eq!(runtime.value("marker").unwrap(), Value::Int(0));
    }
}

#[test]
fn wrong_model_and_non_mapping_view_fail_without_private_representation() {
    let mut runtime = runtime_with_structure(&["A", "B", "C"], &["A", "B", "C"]);
    runtime.states.insert(
        "view".to_string(),
        state(
            sequence("Other", &["A", "B", "C"]),
            ValueType::SequenceLive("Other".to_string()),
        ),
    );

    let error = run_test_transaction(&mut runtime, |runtime| {
        apply_move(runtime, &spec("view", RelativePlacement::After))
    })
    .expect_err("wrong model should fail");
    assert!(error.message.contains("expected live Task"));
    assert!(!error.message.contains("__meld"));

    let mut runtime = runtime_with_structure(&["A", "B", "C"], &["B", "A"]);
    runtime.states.insert(
        "moving".to_string(),
        state(Value::String("B".to_string()), ValueType::String),
    );
    runtime.states.insert(
        "anchor".to_string(),
        state(Value::String("A".to_string()), ValueType::String),
    );
    let error = run_test_transaction(&mut runtime, |runtime| {
        apply_move(runtime, &spec("view", RelativePlacement::After))
    })
    .expect_err("non-mapping view should fail");
    assert!(error.message.contains("does not map"));
    assert!(!error.message.contains("index"));
}
