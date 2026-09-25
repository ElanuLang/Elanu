use elanu_compiler::{
    ast::{Declaration, Expr},
    check_source,
    runtime::{Runtime, Value},
    semantic::ValueType,
};

fn base_source(extra_actions: &str) -> String {
    format!(
        r#"
state model LineItem {{
    state quantity = 1
}}

state model Invoice {{
    state lines: [live LineItem] = []
    derived total = reduce lines from 0 as (total, line) {{
        total + line.quantity
    }}
}}

state itemA: LineItem
state itemB: LineItem
state invoice: Invoice

derived observed = invoice.total
{extra_actions}
"#
    )
}

#[test]
fn owner_sequence_is_a_structural_runtime_value() {
    let source = base_source(
        r#"
action populate {
    invoice.lines = [live itemA, live itemB]
}
"#,
    );
    let checked = check_source(&source).expect("source should check");
    assert_eq!(
        checked.binding_type("__elanu_mseq$invoice$lines"),
        Some(&ValueType::SequenceLive("LineItem".to_string()))
    );

    let mut runtime = Runtime::from_program(&checked).expect("runtime should initialize");
    assert_eq!(
        runtime.value("__elanu_mseq$invoice$lines").unwrap(),
        Value::Sequence {
            element_model: "LineItem".to_string(),
            targets: vec![],
        }
    );
    runtime.run_action("populate").unwrap();
    assert_eq!(
        runtime.value("__elanu_mseq$invoice$lines").unwrap(),
        Value::Sequence {
            element_model: "LineItem".to_string(),
            targets: vec!["itemA".to_string(), "itemB".to_string()],
        }
    );
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(2));
}

#[test]
fn owner_reduction_remains_one_runtime_reducer_in_checked_ast() {
    let source = base_source(
        r#"
action populate {
    invoice.lines = [live itemA]
}
action replace {
    invoice.lines = [live itemA, live itemB]
}
"#,
    );
    let checked = check_source(&source).expect("source should check");
    let runtime_reducers = checked
        .program
        .declarations
        .iter()
        .filter_map(|declaration| match declaration {
            Declaration::Derived(derived) => checked
                .runtime_reduction(&derived.name)
                .map(|payload| (derived, payload)),
            _ => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(runtime_reducers.len(), 1);
    let (derived, payload) = runtime_reducers[0];
    assert_eq!(derived.name, "__elanu_reduce_member$invoice$total");
    assert_eq!(payload.reduction.source, "__elanu_mseq$invoice$lines");
    assert!(!matches!(&derived.expression, Expr::If { .. }));
}

#[test]
fn state_equivalent_sequence_replacement_does_not_invalidate_reducer() {
    let source = base_source(
        r#"
action populate {
    invoice.lines = [live itemA, live itemB]
}
action sameAgain {
    invoice.lines = [live itemA, live itemB]
}
"#,
    );
    let checked = check_source(&source).expect("source should check");
    let mut runtime = Runtime::from_program(&checked).expect("runtime should initialize");

    runtime.run_action("populate").unwrap();
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(2));
    assert_eq!(runtime.derived_evaluations("observed"), Some(1));

    runtime.run_action("sameAgain").unwrap();
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(2));
    assert_eq!(runtime.derived_evaluations("observed"), Some(1));
}

#[test]
fn large_owner_sequence_reduces_at_runtime_without_variant_unrolling() {
    const COUNT: usize = 256;
    let mut source = String::from(
        r#"
state model LineItem {
    state quantity = 1
}

state model Invoice {
    state lines: [live LineItem] = []
    derived total = reduce lines from 0 as (total, line) {
        total + line.quantity
    }
}

"#,
    );
    for index in 0..COUNT {
        source.push_str(&format!("state item{index}: LineItem\n"));
    }
    source.push_str("state invoice: Invoice\n\nderived observed = invoice.total\n\naction populate {\n    invoice.lines = [");
    for index in 0..COUNT {
        if index > 0 {
            source.push_str(", ");
        }
        source.push_str(&format!("live item{index}"));
    }
    source.push_str("]\n}\n");

    let checked = check_source(&source).expect("large source should check");
    let mut runtime = Runtime::from_program(&checked).expect("runtime should initialize");
    runtime.run_action("populate").unwrap();
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(COUNT as i64));

    let runtime_reducers = checked
        .program
        .declarations
        .iter()
        .filter_map(|declaration| match declaration {
            Declaration::Derived(derived) => checked
                .runtime_reduction(&derived.name)
                .map(|payload| (derived, payload)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(runtime_reducers.len(), 1);
    let (derived, payload) = runtime_reducers[0];
    assert_eq!(derived.name, "__elanu_reduce_member$invoice$total");
    assert_eq!(payload.reduction.source, "__elanu_mseq$invoice$lines");
}
