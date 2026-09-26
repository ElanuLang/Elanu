use elanu_compiler::{check_source, parse_source};

const COUNTER: &str = r#"
state count: Int = 0

derived doubled = count * 2

action increment {
    count += 1
}
"#;

#[test]
fn parses_counter_program() {
    let program = parse_source(COUNTER).expect("counter should parse");
    assert_eq!(program.declarations.len(), 3);
}

#[test]
fn checks_counter_program() {
    check_source(COUNTER).expect("counter should pass semantic checks");
}

#[test]
fn rejects_assignment_to_derived_value() {
    let source = r#"
state count = 0
derived doubled = count * 2
action bad {
    doubled = 10
}
"#;

    let errors = check_source(source).expect_err("assignment to derived should fail");
    assert!(errors.iter().any(|error| error
        .message
        .contains("cannot assign to derived value 'doubled'")));
}

#[test]
fn rejects_basic_type_mismatch() {
    let source = r#"
state count: Int = 0
action bad {
    count = "hello"
}
"#;

    let errors = check_source(source).expect_err("type mismatch should fail");
    assert!(errors.iter().any(|error| error
        .message
        .contains("cannot assign String to state 'count' of type Int")));
}

#[test]
fn rejects_compound_assignment_that_widens_int_state_to_float() {
    let source = r#"
state count: Int = 0
action bad {
    count += 1.5
}
"#;

    let errors = check_source(source).expect_err("compound widening should fail");
    assert!(errors.iter().any(|error| error.message.contains(
        "compound arithmetic result Float cannot be stored in state 'count' of type Int"
    )));
}

#[test]
fn parses_conditional_derived_value() {
    let source = r#"
state loggedIn = false
state name = "Matt"

derived greeting =
    if loggedIn {
        "Hello, {name}"
    } else {
        "Sign in"
    }

action login {
    loggedIn = true
}
"#;

    check_source(source).expect("conditional derived example should parse and check");
}

#[test]
fn parses_and_checks_action_call_statement() {
    let source = r#"
state count = 0
action inner {
    count += 1
}
action outer {
    inner()
    count += 10
}
"#;

    check_source(source).expect("action call should parse and check");
}

#[test]
fn action_calls_may_reference_later_action_declarations() {
    let source = r#"
state count = 0
action outer {
    inner()
}
action inner {
    count += 1
}
"#;

    check_source(source).expect("forward action reference should be valid");
}

#[test]
fn rejects_calling_a_non_action() {
    let source = r#"
state count = 0
action bad {
    count()
}
"#;

    let errors = check_source(source).expect_err("state should not be callable as an action");
    assert!(errors
        .iter()
        .any(|error| error.message.contains("'count' is not an action")));
}

#[test]
fn rejects_unknown_action_call() {
    let source = r#"
state count = 0
action bad {
    missing()
}
"#;

    let errors = check_source(source).expect_err("unknown action call should fail");
    assert!(errors
        .iter()
        .any(|error| error.message.contains("unknown action 'missing'")));
}

#[test]
fn parses_and_checks_explicit_fail_statement() {
    let source = r#"
state count = 0
action stop {
    count = 1
    fail "cannot continue"
}
"#;

    check_source(source).expect("explicit fail statement should parse and check");
}

#[test]
fn rejects_non_string_fail_message() {
    let source = r#"
action bad {
    fail 42
}
"#;

    let errors = check_source(source).expect_err("fail message should require String");
    assert!(errors.iter().any(|error| error
        .message
        .contains("fail message must be String, got Int")));
}

#[test]
fn parses_and_checks_if_statement_without_else() {
    let source = r#"
state blocked = true
state count = 0
action update {
    if blocked {
        fail "blocked"
    }
    count = 1
}
"#;

    check_source(source).expect("if statement without else should parse and check");
}

#[test]
fn parses_and_checks_if_else_statement() {
    let source = r#"
state enabled = false
state count = 0
action update {
    if enabled {
        count = 1
    } else {
        count = 2
    }
}
"#;

    check_source(source).expect("if/else statement should parse and check");
}

#[test]
fn rejects_non_bool_if_statement_condition() {
    let source = r#"
state count = 1
action bad {
    if count {
        count = 2
    }
}
"#;

    let errors = check_source(source).expect_err("if statement condition should require Bool");
    assert!(errors.iter().any(|error| error
        .message
        .contains("if statement condition must be Bool, got Int")));
}

#[test]
fn parses_and_checks_numeric_comparisons() {
    let source = r#"
state balance = 10
state threshold = 20
derived atLimit = balance + 10 >= threshold
action withdraw {
    if balance < threshold {
        fail "insufficient balance"
    }
}
"#;

    check_source(source).expect("numeric comparison expressions should parse and check");
}

#[test]
fn parses_and_checks_bool_and_string_equality() {
    let source = r#"
state enabled = true
state name = "Elanu"
derived sameFlag = enabled == true
derived renamed = name != "Other"
"#;

    check_source(source).expect("Bool and String equality should be supported");
}

#[test]
fn rejects_string_ordering() {
    let source = r#"
state first = "a"
state second = "b"
derived ordered = first < second
"#;

    let errors = check_source(source).expect_err("String ordering is not defined in v0.6");
    assert!(errors.iter().any(|error| error
        .message
        .contains("operator Less is not defined for String and String")));
}

#[test]
fn rejects_equality_between_unrelated_primitive_types() {
    let source = r#"
state enabled = true
state count = 1
derived same = enabled == count
"#;

    let errors = check_source(source).expect_err("Bool and Int equality should be rejected");
    assert!(errors.iter().any(|error| error
        .message
        .contains("operator Equal is not defined for Bool and Int")));
}

#[test]
fn semantic_diagnostics_report_statement_source_location() {
    let source = "state count = 0\naction bad {\n    count = \"hello\"\n}\n";

    let errors = check_source(source).expect_err("type mismatch should fail");
    let error = errors
        .iter()
        .find(|error| {
            error
                .message
                .contains("cannot assign String to state 'count' of type Int")
        })
        .expect("expected assignment type diagnostic");

    assert_eq!((error.line, error.column), (3, 5));
}

#[test]
fn displayed_ast_includes_action_parameters_and_arguments() {
    let source = r#"
state quantity = 1

action setQuantity(state target: Int, newValue: Int) {
    target = newValue
}

action update {
    setQuantity(state quantity, quantity + 5)
}
"#;

    let program = parse_source(source).expect("parameterized action program should parse");
    let displayed = program.to_string();

    assert!(
        displayed.contains("ActionParameter State target: Int"),
        "displayed AST should include writable-state parameter:\n{displayed}"
    );
    assert!(
        displayed.contains("ActionParameter Value newValue: Int"),
        "displayed AST should include value parameter:\n{displayed}"
    );
    assert!(
        displayed.contains("StateGrant quantity"),
        "displayed AST should include explicit state-grant argument:\n{displayed}"
    );
    assert!(
        displayed.contains("ValueArgument"),
        "displayed AST should include value argument:\n{displayed}"
    );
    assert!(
        displayed.contains("Binary(Add)"),
        "displayed AST should include the value argument expression:\n{displayed}"
    );
}
