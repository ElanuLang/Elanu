use elanu_compiler::{
    check_source,
    runtime::{Runtime, Value},
};

fn checked_runtime(source: &str) -> Runtime {
    let program = check_source(source).expect("source should parse and check");
    Runtime::from_program(&program).expect("runtime should initialize")
}

#[test]
fn accepts_writable_state_parameter() {
    let source = r#"
state quantity = 1

action increment(state target: Int) {
    target += 1
}

action demo {
    increment(state quantity)
}
"#;

    check_source(source).expect("writable state parameter should be valid");
}

#[test]
fn accepts_value_and_writable_state_parameters_together() {
    let source = r#"
state quantity = 1

action setQuantity(state target: Int, newValue: Int) {
    target = newValue
}

action demo {
    setQuantity(state quantity, quantity + 4)
}
"#;

    check_source(source).expect("mixed state/value parameters should be valid");
}

#[test]
fn value_parameters_are_read_only() {
    let source = r#"
action bad(value: Int) {
    value = 2
}
"#;

    let errors = check_source(source).expect_err("value parameter assignment should fail");

    assert!(
        errors.iter().any(|error| error
            .message
            .contains("cannot assign to value parameter 'value'")),
        "expected read-only value-parameter diagnostic, got {errors:#?}"
    );
}

#[test]
fn writable_state_arguments_require_explicit_state_grant() {
    let source = r#"
state quantity = 1

action increment(state target: Int) {
    target += 1
}

action demo {
    increment(quantity)
}
"#;

    let errors =
        check_source(source).expect_err("ordinary value must not satisfy writable state parameter");

    assert!(
        errors.iter().any(|error| {
            error.message.contains("writable state") && error.message.contains("state quantity")
        }),
        "expected diagnostic suggesting explicit 'state quantity', got {errors:#?}"
    );
}

#[test]
fn derived_values_cannot_be_granted_as_writable_state() {
    let source = r#"
state quantity = 1
derived doubled = quantity * 2

action increment(state target: Int) {
    target += 1
}

action demo {
    increment(state doubled)
}
"#;

    let errors =
        check_source(source).expect_err("derived value must not be grantable as writable state");

    assert!(
        errors.iter().any(|error| {
            error.message.contains("doubled")
                && error.message.contains("derived")
                && error.message.contains("writable")
        }),
        "expected derived-not-writable diagnostic, got {errors:#?}"
    );
}

#[test]
fn checks_action_argument_count() {
    let source = r#"
state quantity = 1

action setQuantity(state target: Int, newValue: Int) {
    target = newValue
}

action demo {
    setQuantity(state quantity)
}
"#;

    let errors = check_source(source).expect_err("wrong action arity should fail");

    assert!(
        errors.iter().any(|error| {
            error.message.contains("setQuantity")
                && error.message.contains("2")
                && error.message.contains("1")
        }),
        "expected action arity diagnostic, got {errors:#?}"
    );
}

#[test]
fn checks_value_parameter_type() {
    let source = r#"
state quantity = 1

action setQuantity(state target: Int, newValue: Int) {
    target = newValue
}

action demo {
    setQuantity(state quantity, "five")
}
"#;

    let errors = check_source(source).expect_err("wrong value argument type should fail");

    assert!(
        errors.iter().any(|error| {
            error.message.contains("String")
                && error.message.contains("Int")
                && error.message.contains("newValue")
        }),
        "expected value-parameter type diagnostic, got {errors:#?}"
    );
}

#[test]
fn checks_writable_state_parameter_type() {
    let source = r#"
state label = "one"

action increment(state target: Int) {
    target += 1
}

action demo {
    increment(state label)
}
"#;

    let errors = check_source(source).expect_err("wrong state argument type should fail");

    assert!(
        errors.iter().any(|error| {
            error.message.contains("label")
                && error.message.contains("String")
                && error.message.contains("Int")
        }),
        "expected writable-state type diagnostic, got {errors:#?}"
    );
}

#[test]
fn int_value_argument_may_widen_to_float_parameter() {
    let source = r#"
state result: Float = 0.0

action setResult(state target: Float, value: Float) {
    target = value
}

action demo {
    setResult(state result, 5)
}
"#;

    check_source(source)
        .expect("existing Int-to-Float assignability should apply to value parameters");
}

#[test]
fn writable_state_authority_can_be_forwarded() {
    let source = r#"
state quantity = 1

action increment(state target: Int) {
    target += 1
}

action incrementTwice(state target: Int) {
    increment(state target)
    increment(state target)
}

action demo {
    incrementTwice(state quantity)
}
"#;

    check_source(source).expect("writable state authority should be explicitly forwardable");
}

#[test]
fn parameterized_action_can_mutate_different_caller_selected_states() {
    let mut runtime = checked_runtime(
        r#"
state a = 1
state b = 10

action increment(state target: Int) {
    target += 1
}

action demo {
    increment(state a)
    increment(state b)
}
"#,
    );

    runtime.run_action("demo").unwrap();

    assert_eq!(runtime.value("a").unwrap(), Value::Int(2));
    assert_eq!(runtime.value("b").unwrap(), Value::Int(11));
}

#[test]
fn value_arguments_are_evaluated_in_the_callers_transaction_world() {
    let mut runtime = checked_runtime(
        r#"
state source = 10
state target = 0

action copy(state destination: Int, value: Int) {
    destination = value
}

action demo {
    source = 20
    copy(state target, source + 5)
}
"#,
    );

    runtime.run_action("demo").unwrap();

    assert_eq!(runtime.value("source").unwrap(), Value::Int(20));
    assert_eq!(runtime.value("target").unwrap(), Value::Int(25));
}

#[test]
fn failure_through_writable_state_parameter_rolls_back_outer_transaction() {
    let mut runtime = checked_runtime(
        r#"
state a = 1
state b = 10

action changeThenFail(state target: Int) {
    target += 1
    fail "reject"
}

action demo {
    a = 20
    changeThenFail(state b)
}
"#,
    );

    let error = runtime
        .run_action("demo")
        .expect_err("nested failure should abort the shared transaction");

    assert!(error.message.contains("reject"));
    assert_eq!(runtime.value("a").unwrap(), Value::Int(1));
    assert_eq!(runtime.value("b").unwrap(), Value::Int(10));
}

#[test]
fn existing_zero_parameter_action_syntax_remains_valid() {
    let mut runtime = checked_runtime(
        r#"
state count = 0

action increment {
    count += 1
}

action demo {
    increment()
}
"#,
    );

    runtime.run_action("demo").unwrap();

    assert_eq!(runtime.value("count").unwrap(), Value::Int(1));
}

#[test]
fn parses_parameterized_action_declaration_syntax() {
    let source = r#"
action setQuantity(state target: Int, newValue: Int) {
    target = newValue
}
"#;

    elanu_compiler::parse_source(source)
        .expect("parameterized action declaration syntax should parse");
}

#[test]
fn parses_explicit_state_grant_argument_syntax() {
    let source = r#"
state quantity = 1

action increment(state target: Int) {
    target += 1
}

action demo {
    increment(state quantity)
}
"#;

    elanu_compiler::parse_source(source)
        .expect("explicit writable-state grant syntax should parse");
}

#[test]
fn state_parameter_may_share_name_with_underlying_state() {
    let mut runtime = checked_runtime(
        r#"
state count = 1

action increment(state count: Int) {
    count += 1
}

action demo {
    increment(state count)
}
"#,
    );

    runtime.run_action("demo").unwrap();

    assert_eq!(runtime.value("count").unwrap(), Value::Int(2));
}

#[test]
fn action_parameters_do_not_shadow_globals_inside_derived_evaluation() {
    let mut runtime = checked_runtime(
        r#"
state count = 1
state observed = 0
derived doubled = count * 2

action capture(count: Int) {
    observed = doubled
}

action demo {
    capture(10)
}
"#,
    );

    runtime.run_action("demo").unwrap();

    assert_eq!(runtime.value("count").unwrap(), Value::Int(1));
    assert_eq!(runtime.value("doubled").unwrap(), Value::Int(2));
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(2));
}

#[test]
fn rejects_parenthesized_empty_action_parameter_list() {
    let source = r#"
action noop() {
}
"#;

    assert!(
        elanu_compiler::parse_source(source).is_err(),
        "zero-parameter action declarations must omit parentheses"
    );
}

#[test]
fn rejects_unknown_action_parameter_type() {
    let source = r#"
action whatever(value: Banana) {
}
"#;

    let errors =
        check_source(source).expect_err("unknown action parameter type should be rejected");

    assert!(
        errors.iter().any(|error| {
            error.message.contains("Banana")
                && error.message.contains("unknown")
                && error.message.contains("type")
        }),
        "expected unknown-type diagnostic for Banana, got {errors:#?}"
    );
}

#[test]
fn rejects_unknown_writable_state_parameter_type() {
    let source = r#"
action whatever(state target: Banana) {
}
"#;

    let errors =
        check_source(source).expect_err("unknown writable state parameter type should be rejected");

    assert!(
        errors.iter().any(|error| {
            error.message.contains("Banana")
                && error.message.contains("unknown")
                && error.message.contains("type")
        }),
        "expected unknown-type diagnostic for Banana, got {errors:#?}"
    );
}

#[test]
fn rejects_unknown_state_annotation_type() {
    let source = r#"
state value: Banana = 1
"#;

    let errors =
        check_source(source).expect_err("unknown state annotation type should be rejected");

    assert!(
        errors.iter().any(|error| {
            error.message.contains("Banana")
                && error.message.contains("unknown")
                && error.message.contains("type")
        }),
        "expected unknown-type diagnostic for Banana, got {errors:#?}"
    );
}
