use elanu_compiler::{
    ast::{Declaration, Expr},
    check_source,
    runtime::{Runtime, Value},
};

fn checked_runtime(source: &str) -> Runtime {
    let program = check_source(source).expect("source should parse and check");
    Runtime::from_program(&program).expect("runtime should initialize")
}

#[test]
fn model_local_reduction_reads_earlier_owner_state_and_derived_members() {
    let mut runtime = checked_runtime(
        r#"
state model LineItem {
    state quantity = 1
}

state model Invoice {
    state minimumBase = 1
    derived minimum = minimumBase + 1
    state lines: [live LineItem] = []
    derived qualifyingTotal = reduce lines from 0 as (total, line) {
        if line.quantity >= minimum {
            total + line.quantity
        } else {
            total
        }
    }
}

state itemA: LineItem
state itemB: LineItem
state invoice: Invoice

derived observed = invoice.qualifyingTotal

action populate {
    itemA.quantity = 1
    itemB.quantity = 3
    invoice.lines = [live itemA, live itemB]
}

action lowerMinimum {
    invoice.minimumBase = 0
}
"#,
    );

    runtime.run_action("populate").unwrap();
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(3));
    assert_eq!(runtime.derived_evaluations("observed"), Some(1));

    runtime.run_action("lowerMinimum").unwrap();
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(4));
    assert_eq!(runtime.derived_evaluations("observed"), Some(2));
}

#[test]
fn model_local_reduction_initial_value_may_read_earlier_owner_state() {
    let mut runtime = checked_runtime(
        r#"
state model LineItem {
    state quantity = 1
}

state model Invoice {
    state base = 5
    state lines: [live LineItem] = []
    derived total = reduce lines from base as (total, line) {
        total + line.quantity
    }
}

state invoice: Invoice

derived observed = invoice.total

action raiseBase {
    invoice.base = 7
}
"#,
    );

    assert_eq!(runtime.value("observed").unwrap(), Value::Int(5));
    runtime.run_action("raiseBase").unwrap();
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(7));
}

#[test]
fn owner_local_reduction_observes_staged_filter_state_and_rolls_back() {
    let mut runtime = checked_runtime(
        r#"
state model LineItem {
    state quantity = 3
}

state model Invoice {
    state minimum = 1
    state lines: [live LineItem] = []
    derived qualifyingTotal = reduce lines from 0 as (total, line) {
        if line.quantity >= minimum {
            total + line.quantity
        } else {
            total
        }
    }
}

state itemA: LineItem
state invoice: Invoice
state seen = 0

derived observed = invoice.qualifyingTotal

action setup {
    invoice.lines = [live itemA]
}

action stageFilter {
    invoice.minimum = 4
    seen = invoice.qualifyingTotal
}

action failFilter {
    invoice.minimum = 1
    seen = invoice.qualifyingTotal
    fail "rollback"
}
"#,
    );

    runtime.run_action("setup").unwrap();
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(3));

    runtime.run_action("stageFilter").unwrap();
    assert_eq!(runtime.value("seen").unwrap(), Value::Int(0));
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(0));

    assert!(runtime.run_action("failFilter").is_err());
    assert_eq!(runtime.value("seen").unwrap(), Value::Int(0));
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(0));
}

#[test]
fn runtime_reduction_construction_uses_checked_payload_not_derived_expression() {
    let mut program = check_source(
        r#"
state model LineItem {
    state quantity = 2
}

state model Invoice {
    state lines: [live LineItem] = []
    derived total = reduce lines from 0 as (total, line) {
        total + line.quantity
    }
}

state item: LineItem
state invoice: Invoice

derived observed = invoice.total

action setup {
    invoice.lines = [live item]
}
"#,
    )
    .expect("source should parse and check");

    let reduction_name = program
        .program
        .declarations
        .iter()
        .find_map(|declaration| match declaration {
            Declaration::Derived(derived) if program.runtime_reduction(&derived.name).is_some() => {
                Some(derived.name.clone())
            }
            _ => None,
        })
        .expect("lowered program should contain a runtime reduction binding");

    let reduction_expression = program
        .program
        .declarations
        .iter()
        .find_map(|declaration| match declaration {
            Declaration::Derived(derived) if derived.name == reduction_name => {
                Some(&derived.expression)
            }
            _ => None,
        })
        .expect("runtime reduction binding should remain in the lowered program");
    assert!(!matches!(
        reduction_expression,
        Expr::String(value) if value.starts_with("__elanu_runtime_reduce$")
    ));

    for declaration in &mut program.program.declarations {
        if let Declaration::Derived(derived) = declaration {
            if derived.name == reduction_name {
                derived.expression = Expr::String("marker transport intentionally removed".into());
            }
        }
    }

    let mut runtime = Runtime::from_program(&program)
        .expect("runtime should initialize from the checked reduction payload");
    runtime.run_action("setup").unwrap();
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(2));
}

#[test]
fn empty_owner_local_reduction_still_rejects_float_step_for_int_accumulator() {
    let errors = check_source(
        r#"
state model LineItem {
    state price = 1.5
}

state model Invoice {
    state lines: [live LineItem] = []
    derived total = reduce lines from 0 as (total, line) {
        total + line.price
    }
}

state invoice: Invoice
"#,
    )
    .expect_err("empty sequence must not bypass reduction step typing");

    assert!(errors.iter().any(|error| {
        error.message.contains("step has type Float")
            && error.message.contains("accumulator")
            && error.message.contains("Int")
    }));
}

#[test]
fn float_reduction_normalizes_int_step_before_next_iteration() {
    let mut runtime = checked_runtime(
        r#"
state model LineItem {
    state quantity = 1
}

state model Invoice {
    state lines: [live LineItem] = []
    derived folded = reduce lines from 4.0 as (total, line) {
        if line.quantity == 1 {
            line.quantity
        } else {
            total / 2
        }
    }
}

state itemA: LineItem
state itemB: LineItem
state invoice: Invoice

derived observed = invoice.folded

action setup {
    itemB.quantity = 2
    invoice.lines = [live itemA, live itemB]
}
"#,
    );

    runtime.run_action("setup").unwrap();
    assert_eq!(runtime.value("observed").unwrap(), Value::Float(0.5));
}

#[test]
fn model_local_reduction_rejects_forward_owner_member_capture() {
    let errors = check_source(
        r#"
state model LineItem {
    state quantity = 1
}

state model Invoice {
    state lines: [live LineItem] = []
    derived qualifyingTotal = reduce lines from 0 as (total, line) {
        if line.quantity >= minimum {
            total + line.quantity
        } else {
            total
        }
    }
    state minimum = 1
}

state invoice: Invoice
"#,
    )
    .expect_err("forward owner member capture must remain illegal");

    assert!(errors.iter().any(|error| {
        error.message.contains("rooted model-local reduction")
            && error.message.contains("earlier ordinary member")
    }));
}
