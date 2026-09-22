use elanu_compiler::{
    check_source,
    runtime::{Runtime, Value},
};

fn runtime(source: &str) -> Runtime {
    let program = check_source(source).expect("source should parse and check");
    Runtime::from_program(&program).expect("runtime should initialize")
}

#[test]
fn runs_counter_action_and_updates_derived_value() {
    let mut runtime = runtime(
        r#"
state count: Int = 0
derived doubled = count * 2
action increment {
    count += 1
}
"#,
    );

    assert_eq!(runtime.value("count").unwrap(), Value::Int(0));
    assert_eq!(runtime.value("doubled").unwrap(), Value::Int(0));

    runtime.run_action("increment").unwrap();

    assert_eq!(runtime.value("count").unwrap(), Value::Int(1));
    assert_eq!(runtime.value("doubled").unwrap(), Value::Int(2));
}

#[test]
fn derived_reads_inside_action_see_staged_state() {
    let mut runtime = runtime(
        r#"
state price = 10
state quantity = 2
state observed = 0
derived total = price * quantity
action update {
    price = 20
    observed = total
}
"#,
    );

    runtime.run_action("update").unwrap();

    assert_eq!(runtime.value("price").unwrap(), Value::Int(20));
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(40));
    assert_eq!(runtime.value("total").unwrap(), Value::Int(40));
}

#[test]
fn no_op_write_does_not_invalidate_committed_derived_cache() {
    let mut runtime = runtime(
        r#"
state count = 5
derived doubled = count * 2
action same {
    count = 5
}
"#,
    );

    assert_eq!(runtime.value("doubled").unwrap(), Value::Int(10));
    assert_eq!(runtime.derived_evaluations("doubled"), Some(1));

    runtime.run_action("same").unwrap();

    assert_eq!(runtime.value("doubled").unwrap(), Value::Int(10));
    assert_eq!(runtime.derived_evaluations("doubled"), Some(1));
}

#[test]
fn conditional_dependencies_are_replaced_when_branch_changes() {
    let mut runtime = runtime(
        r#"
state useA = true
state a = 1
state b = 2
derived selected = if useA { a } else { b }
action switch {
    useA = false
}
action changeA {
    a = 10
}
"#,
    );

    assert_eq!(runtime.value("selected").unwrap(), Value::Int(1));
    assert_eq!(runtime.derived_evaluations("selected"), Some(1));

    runtime.run_action("switch").unwrap();
    assert_eq!(runtime.value("selected").unwrap(), Value::Int(2));
    assert_eq!(runtime.derived_evaluations("selected"), Some(2));

    runtime.run_action("changeA").unwrap();
    assert_eq!(runtime.value("selected").unwrap(), Value::Int(2));
    assert_eq!(runtime.derived_evaluations("selected"), Some(2));
}

#[test]
fn runtime_failure_rolls_back_all_staged_writes() {
    let mut runtime = runtime(
        r#"
state count = 1
action overflow {
    count = 10
    count += 9223372036854775807
}
"#,
    );

    let error = runtime
        .run_action("overflow")
        .expect_err("overflow should abort the action");
    assert!(error.message.contains("overflow"));
    assert_eq!(runtime.value("count").unwrap(), Value::Int(1));
}

#[test]
fn unknown_action_is_a_runtime_error() {
    let mut runtime = runtime("state count = 0\n");
    let error = runtime
        .run_action("missing")
        .expect_err("missing action should fail");
    assert!(error.message.contains("unknown action 'missing'"));
}

#[test]
fn snapshot_is_deterministic_and_in_declaration_order() {
    let mut runtime = runtime(
        r#"
state a = 1
state b = 2
derived total = a + b
"#,
    );

    let snapshot = runtime.snapshot().unwrap();
    let names: Vec<_> = snapshot.iter().map(|entry| entry.name.as_str()).collect();
    assert_eq!(names, vec!["a", "b", "total"]);
}

#[test]
fn snapshot_preserves_interleaved_state_and_derived_declaration_order() {
    let mut runtime = runtime(
        r#"
state a = 1
derived x = a + 1
state b = 2
derived y = b + 1
"#,
    );

    let snapshot = runtime.snapshot().unwrap();
    let names: Vec<_> = snapshot.iter().map(|entry| entry.name.as_str()).collect();
    assert_eq!(names, vec!["a", "x", "b", "y"]);
}

#[test]
fn inferred_float_state_uses_static_common_type_even_when_int_branch_initializes() {
    let mut runtime = runtime(
        r#"
state chooseInt = true
state value = if chooseInt { 1 } else { 2.5 }
action later {
    value = 3.5
}
"#,
    );

    assert_eq!(runtime.value("value").unwrap(), Value::Float(1.0));
    runtime.run_action("later").unwrap();
    assert_eq!(runtime.value("value").unwrap(), Value::Float(3.5));
}

#[test]
fn inferred_float_derived_uses_static_common_type_even_when_int_branch_runs() {
    let mut runtime = runtime(
        r#"
state chooseInt = true
derived value = if chooseInt { 1 } else { 2.5 }
"#,
    );

    assert_eq!(runtime.value("value").unwrap(), Value::Float(1.0));
}

#[test]
fn nested_action_calls_share_one_transaction() {
    let mut runtime = runtime(
        r#"
state count = 0
action inner {
    count += 1
}
action outer {
    inner()
    count += 10
}
"#,
    );

    runtime.run_action("outer").unwrap();
    assert_eq!(runtime.value("count").unwrap(), Value::Int(11));
}

#[test]
fn derived_reads_after_nested_action_see_staged_writes() {
    let mut runtime = runtime(
        r#"
state count = 0
state observed = 0
derived doubled = count * 2
action inner {
    count += 1
}
action outer {
    inner()
    observed = doubled
}
"#,
    );

    runtime.run_action("outer").unwrap();
    assert_eq!(runtime.value("count").unwrap(), Value::Int(1));
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(2));
    assert_eq!(runtime.value("doubled").unwrap(), Value::Int(2));
}

#[test]
fn nested_action_runtime_failure_rolls_back_outer_transaction() {
    let mut runtime = runtime(
        r#"
state count = 5
action inner {
    count = 20
    count /= 0
}
action outer {
    count = 10
    inner()
    count = 30
}
"#,
    );

    let error = runtime
        .run_action("outer")
        .expect_err("nested division by zero should abort the outer action");
    assert!(error.message.contains("division by zero"));
    assert_eq!(runtime.value("count").unwrap(), Value::Int(5));
}

#[test]
fn recursive_action_call_fails_safely() {
    let mut runtime = runtime(
        r#"
state count = 0
action first {
    second()
}
action second {
    first()
}
"#,
    );

    let error = runtime
        .run_action("first")
        .expect_err("recursive action cycle should fail instead of overflowing the stack");
    assert!(error.message.contains("recursive action call"));
    assert!(error.message.contains("first -> second -> first"));
    assert_eq!(runtime.value("count").unwrap(), Value::Int(0));
}

#[test]
fn explicit_fail_rolls_back_staged_writes() {
    let mut runtime = runtime(
        r#"
state count = 5
action stop {
    count = 10
    fail "cannot continue"
    count = 20
}
"#,
    );

    let error = runtime
        .run_action("stop")
        .expect_err("explicit fail should abort the action");
    assert!(error.message.contains("cannot continue"));
    assert_eq!(runtime.value("count").unwrap(), Value::Int(5));
}

#[test]
fn nested_explicit_fail_rolls_back_outer_transaction() {
    let mut runtime = runtime(
        r#"
state count = 5
action inner {
    count = 20
    fail "inner rejected the transition"
}
action outer {
    count = 10
    inner()
    count = 30
}
"#,
    );

    let error = runtime
        .run_action("outer")
        .expect_err("nested explicit fail should abort the outer action");
    assert!(error.message.contains("inner rejected the transition"));
    assert_eq!(runtime.value("count").unwrap(), Value::Int(5));
}

#[test]
fn if_statement_selects_the_correct_branch() {
    let mut runtime = runtime(
        r#"
state enabled = false
state count = 0
action choose {
    if enabled {
        count = 1
    } else {
        count = 2
    }
}
action enable {
    enabled = true
}
"#,
    );

    runtime.run_action("choose").unwrap();
    assert_eq!(runtime.value("count").unwrap(), Value::Int(2));

    runtime.run_action("enable").unwrap();
    runtime.run_action("choose").unwrap();
    assert_eq!(runtime.value("count").unwrap(), Value::Int(1));
}

#[test]
fn if_statement_condition_sees_staged_state() {
    let mut runtime = runtime(
        r#"
state enabled = false
state count = 0
action enableAndRun {
    enabled = true
    if enabled {
        count = 1
    } else {
        count = 2
    }
}
"#,
    );

    runtime.run_action("enableAndRun").unwrap();
    assert_eq!(runtime.value("enabled").unwrap(), Value::Bool(true));
    assert_eq!(runtime.value("count").unwrap(), Value::Int(1));
}

#[test]
fn conditional_fail_rolls_back_the_action() {
    let mut runtime = runtime(
        r#"
state blocked = true
state count = 5
action update {
    count = 10
    if blocked {
        fail "blocked"
    }
    count = 20
}
"#,
    );

    let error = runtime
        .run_action("update")
        .expect_err("selected fail branch should abort the action");
    assert!(error.message.contains("blocked"));
    assert_eq!(runtime.value("count").unwrap(), Value::Int(5));
}

#[test]
fn numeric_comparison_condition_sees_staged_state() {
    let mut runtime = runtime(
        r#"
state count = 0
state result = 0
action update {
    count = 5
    if count >= 5 {
        result = 1
    } else {
        result = 2
    }
}
"#,
    );

    runtime.run_action("update").unwrap();
    assert_eq!(runtime.value("count").unwrap(), Value::Int(5));
    assert_eq!(runtime.value("result").unwrap(), Value::Int(1));
}

#[test]
fn primitive_equality_produces_bool_values() {
    let mut runtime = runtime(
        r#"
state enabled = true
state name = "Meld"
derived enabledMatches = enabled == true
derived nameMatches = name == "Meld"
derived nameDiffers = name != "Other"
"#,
    );

    assert_eq!(runtime.value("enabledMatches").unwrap(), Value::Bool(true));
    assert_eq!(runtime.value("nameMatches").unwrap(), Value::Bool(true));
    assert_eq!(runtime.value("nameDiffers").unwrap(), Value::Bool(true));
}

#[test]
fn mixed_numeric_comparison_does_not_round_large_int_to_float() {
    let mut runtime = runtime(
        r#"
state large = 9007199254740993
state nearby = 9007199254740992.0
derived greater = large > nearby
derived equal = large == nearby
"#,
    );

    assert_eq!(runtime.value("greater").unwrap(), Value::Bool(true));
    assert_eq!(runtime.value("equal").unwrap(), Value::Bool(false));
}

#[test]
fn comparison_precedence_is_below_arithmetic() {
    let mut runtime = runtime(
        r#"
derived result = 2 + 3 * 4 == 14
"#,
    );

    assert_eq!(runtime.value("result").unwrap(), Value::Bool(true));
}

#[test]
fn transaction_local_derived_cache_invalidates_after_later_staged_write() {
    let mut runtime = runtime(
        r#"
state price = 10
state quantity = 2
state observedBefore = 0
state observedAfter = 0
derived total = price * quantity
action update {
    observedBefore = total
    price = 20
    observedAfter = total
}
"#,
    );

    runtime.run_action("update").unwrap();

    assert_eq!(runtime.value("observedBefore").unwrap(), Value::Int(20));
    assert_eq!(runtime.value("observedAfter").unwrap(), Value::Int(40));
    assert_eq!(runtime.derived_evaluations("total"), Some(2));
}

#[test]
fn rollback_discards_transaction_local_derived_world() {
    let mut runtime = runtime(
        r#"
state price = 10
state quantity = 2
state observed = 0
derived total = price * quantity
action abort {
    price = 20
    observed = total
    fail "abort"
}
"#,
    );

    assert_eq!(runtime.value("total").unwrap(), Value::Int(20));

    let error = runtime
        .run_action("abort")
        .expect_err("explicit failure should roll back transaction");
    assert!(error.message.contains("abort"));

    assert_eq!(runtime.value("price").unwrap(), Value::Int(10));
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(0));
    assert_eq!(runtime.value("total").unwrap(), Value::Int(20));
}

#[test]
fn failed_derived_evaluation_is_not_cached_and_retries() {
    let mut runtime = runtime(
        r#"
state numerator = 42
state denominator = 0
derived answer = numerator / denominator
action ready {
    denominator = 1
}
"#,
    );

    let first = runtime
        .value("answer")
        .expect_err("first derived evaluation should fail");
    assert!(first.message.contains("division by zero"));

    let second = runtime
        .value("answer")
        .expect_err("failed derived evaluation must be retried");
    assert!(second.message.contains("division by zero"));

    assert_eq!(runtime.derived_evaluations("answer"), Some(0));

    runtime.run_action("ready").unwrap();

    assert_eq!(runtime.value("answer").unwrap(), Value::Int(42));
    assert_eq!(runtime.derived_evaluations("answer"), Some(1));
}

#[test]
fn derived_chain_invalidation_propagates_transitively() {
    let mut runtime = runtime(
        r#"
state count = 2
derived doubled = count * 2
derived plusOne = doubled + 1
action update {
    count = 3
}
"#,
    );

    assert_eq!(runtime.value("plusOne").unwrap(), Value::Int(5));
    assert_eq!(runtime.derived_evaluations("doubled"), Some(1));
    assert_eq!(runtime.derived_evaluations("plusOne"), Some(1));

    runtime.run_action("update").unwrap();

    assert_eq!(runtime.value("plusOne").unwrap(), Value::Int(7));
    assert_eq!(runtime.derived_evaluations("doubled"), Some(2));
    assert_eq!(runtime.derived_evaluations("plusOne"), Some(2));
}

#[test]
fn diamond_dependency_recomputes_each_node_once() {
    let mut runtime = runtime(
        r#"
state a = 1
derived b = a + 1
derived c = a + 2
derived d = b + c
action update {
    a = 10
}
"#,
    );

    assert_eq!(runtime.value("d").unwrap(), Value::Int(5));
    assert_eq!(runtime.derived_evaluations("b"), Some(1));
    assert_eq!(runtime.derived_evaluations("c"), Some(1));
    assert_eq!(runtime.derived_evaluations("d"), Some(1));

    runtime.run_action("update").unwrap();

    assert_eq!(runtime.value("d").unwrap(), Value::Int(23));
    assert_eq!(runtime.derived_evaluations("b"), Some(2));
    assert_eq!(runtime.derived_evaluations("c"), Some(2));
    assert_eq!(runtime.derived_evaluations("d"), Some(2));
}

#[test]
fn unselected_if_expression_branch_is_not_read_or_evaluated() {
    let mut runtime = runtime(
        r#"
state chooseRisky = false
state denominator = 0
derived result = if chooseRisky { 10 / denominator } else { 42 }
action changeDenominator {
    denominator = 1
}
action enableRisky {
    chooseRisky = true
}
"#,
    );

    // The division-by-zero branch is not selected.
    assert_eq!(runtime.value("result").unwrap(), Value::Int(42));
    assert_eq!(runtime.derived_evaluations("result"), Some(1));

    // denominator was not read, so changing it must not invalidate result.
    runtime.run_action("changeDenominator").unwrap();

    assert_eq!(runtime.value("result").unwrap(), Value::Int(42));
    assert_eq!(runtime.derived_evaluations("result"), Some(1));

    // Changing the actual dependency selects the other branch. The newly
    // selected branch then reads denominator and evaluates normally.
    runtime.run_action("enableRisky").unwrap();

    assert_eq!(runtime.value("result").unwrap(), Value::Int(10));
    assert_eq!(runtime.derived_evaluations("result"), Some(2));
}
