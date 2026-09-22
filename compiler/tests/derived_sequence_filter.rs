use elanu_compiler::{
    check_source,
    runtime::{Runtime, Value},
};

fn checked_runtime(source: &str) -> Runtime {
    let program = check_source(source).expect("source should parse and check");
    Runtime::from_program(&program).expect("runtime should initialize")
}

#[test]
fn filter_preserves_order_identity_and_multiplicity_and_feeds_reduction() {
    let mut runtime = checked_runtime(
        r#"
state model LineItem {
    state quantity = 0
}

state model Invoice {
    state lines: [live LineItem] = []
    derived activeLines = filter lines as line {
        line.quantity > 0
    }
    derived activeTotal = reduce activeLines from 0 as (total, line) {
        total + line.quantity
    }
}

state itemA: LineItem
state itemB: LineItem
state itemC: LineItem
state invoice: Invoice

derived observed = invoice.activeTotal

action setup {
    itemA.quantity = 2
    itemB.quantity = 0
    itemC.quantity = 5
    invoice.lines = [live itemA, live itemB, live itemA, live itemC]
}
"#,
    );

    runtime.run_action("setup").unwrap();

    assert_eq!(
        runtime
            .value("__meld_filter_member$invoice$activeLines")
            .unwrap(),
        Value::Sequence {
            element_model: "LineItem".to_string(),
            targets: vec![
                "itemA".to_string(),
                "itemA".to_string(),
                "itemC".to_string(),
            ],
        }
    );
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(9));
}

#[test]
fn child_predicate_change_updates_membership_and_downstream_reduction() {
    let mut runtime = checked_runtime(
        r#"
state model LineItem {
    state quantity = 0
}

state model Invoice {
    state lines: [live LineItem] = []
    derived activeLines = filter lines as line {
        line.quantity > 0
    }
    derived activeTotal = reduce activeLines from 0 as (total, line) {
        total + line.quantity
    }
}

state itemA: LineItem
state itemB: LineItem
state invoice: Invoice

derived observed = invoice.activeTotal

action setup {
    itemA.quantity = 2
    invoice.lines = [live itemA, live itemB]
}

action activateB {
    itemB.quantity = 3
}

action deactivateA {
    itemA.quantity = 0
}
"#,
    );

    runtime.run_action("setup").unwrap();
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(2));

    runtime.run_action("activateB").unwrap();
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(5));

    runtime.run_action("deactivateA").unwrap();
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(3));
}

#[test]
fn filter_tracks_only_child_facts_its_predicate_reads() {
    let mut runtime = checked_runtime(
        r#"
state model LineItem {
    state quantity = 1
    state note = 0
}

state model Invoice {
    state lines: [live LineItem] = []
    derived activeLines = filter lines as line {
        line.quantity > 0
    }
    derived activeCount = reduce activeLines from 0 as (count, line) {
        count + 1
    }
}

state item: LineItem
state invoice: Invoice

derived observed = invoice.activeCount

action setup {
    invoice.lines = [live item]
}

action changeNote {
    item.note = 9
}
"#,
    );

    runtime.run_action("setup").unwrap();
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(1));
    let filter_name = "__meld_filter_member$invoice$activeLines";
    assert_eq!(runtime.derived_evaluations(filter_name), Some(1));

    runtime.run_action("changeNote").unwrap();
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(1));
    assert_eq!(runtime.derived_evaluations(filter_name), Some(1));
}

#[test]
fn filter_reads_earlier_owner_state_and_sees_transaction_local_changes() {
    let mut runtime = checked_runtime(
        r#"
state model LineItem {
    state quantity = 3
}

state model Invoice {
    state minimum = 1
    state lines: [live LineItem] = []
    derived activeLines = filter lines as line {
        line.quantity >= minimum
    }
    derived activeTotal = reduce activeLines from 0 as (total, line) {
        total + line.quantity
    }
}

state item: LineItem
state invoice: Invoice
state seen = 0

derived observed = invoice.activeTotal

action setup {
    invoice.lines = [live item]
}

action raiseMinimumAndRead {
    invoice.minimum = 4
    seen = invoice.activeTotal
}

action lowerMinimumThenFail {
    invoice.minimum = 1
    seen = invoice.activeTotal
    fail "rollback"
}
"#,
    );

    runtime.run_action("setup").unwrap();
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(3));

    runtime.run_action("raiseMinimumAndRead").unwrap();
    assert_eq!(runtime.value("seen").unwrap(), Value::Int(0));
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(0));

    assert!(runtime.run_action("lowerMinimumThenFail").is_err());
    assert_eq!(runtime.value("seen").unwrap(), Value::Int(0));
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(0));
}

#[test]
fn filter_predicate_must_be_bool() {
    let errors = check_source(
        r#"
state model LineItem {
    state quantity = 1
}

state model Invoice {
    state lines: [live LineItem] = []
    derived activeLines = filter lines as line {
        line.quantity
    }
}

state invoice: Invoice
"#,
    )
    .expect_err("non-Bool filter predicate must be rejected");

    assert!(errors.iter().any(|error| {
        error.message.contains("predicate must be Bool") && error.message.contains("Int")
    }));
}

#[test]
fn filter_rejects_forward_owner_member_capture() {
    let errors = check_source(
        r#"
state model LineItem {
    state quantity = 1
}

state model Invoice {
    state lines: [live LineItem] = []
    derived activeLines = filter lines as line {
        line.quantity >= minimum
    }
    state minimum = 1
}

state invoice: Invoice
"#,
    )
    .expect_err("forward owner member capture must remain illegal");

    assert!(errors.iter().any(|error| {
        error
            .message
            .contains("rooted model-local filter predicate")
            && error.message.contains("earlier ordinary owner member")
    }));
}
