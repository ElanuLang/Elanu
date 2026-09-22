use elanu_compiler::parse_source;

#[test]
fn parse_display_does_not_expose_sequence_or_reduction_markers() {
    let source = r#"state model Invoice {
    state quantity = 1
}

state invoiceA: Invoice
state invoiceB: Invoice
state order: [live Invoice] = [live invoiceA, live invoiceB]
derived firstQuantity = order[0].quantity
derived total = reduce order from 0 as (sum, invoice) {
    sum + invoice.quantity
}
"#;

    let program = parse_source(source).expect("surface program should parse");
    let displayed = format!("{program}");

    assert!(
        !displayed.contains("__meld_"),
        "source-facing AST display leaked a private marker:\n{displayed}"
    );
    assert!(displayed.contains("StateDecl order: [live Invoice]"));
    assert!(displayed.contains("SequenceLiteral"));
    assert!(displayed.contains("Live(invoiceA)"));
    assert!(displayed.contains("Live(invoiceB)"));
    assert!(displayed.contains("Name(order[0].quantity)"));
    assert!(displayed.contains("Reduction"));
    assert!(displayed.contains("Source(order)"));
    assert!(displayed.contains("InitialSource(\"0\")"));
    assert!(displayed.contains("Bindings(sum, invoice)"));
    assert!(displayed.contains("StepSource(\"sum + invoice.quantity\")"));
}

#[test]
fn parse_display_decodes_state_model_sequence_member_types_and_literals() {
    let source = r#"state model LineItem {
    state quantity = 1
}

state model Invoice {
    state lines: [live LineItem] = []
}
"#;

    let program = parse_source(source).expect("state-model sequence member should parse");
    let displayed = format!("{program}");

    assert!(!displayed.contains("__meld_"), "{displayed}");
    assert!(displayed.contains("StateDecl lines: [live LineItem]"));
    assert!(displayed.contains("SequenceLiteral"));
}

#[test]
fn sequence_surface_error_reports_the_original_bracket_location() {
    let bad_line = "state order: [live Invoice] = [live invoiceA, 1]";
    let source = format!(
        "state model Invoice {{\n    state quantity = 1\n}}\nstate invoiceA: Invoice\n{bad_line}\n"
    );

    let errors = parse_source(&source).expect_err("mixed sequence literal should be rejected");
    let error = errors
        .iter()
        .find(|error| error.message.contains("ordered-sequence literals"))
        .expect("expected ordered-sequence literal diagnostic");

    assert_eq!(error.line, 5);
    assert_eq!(
        error.column,
        bad_line.rfind('[').expect("literal bracket") + 1
    );
}

#[test]
fn multiline_reduction_preserves_locations_on_following_lines() {
    let source = r#"state model Invoice {
    state quantity = 1
}
state invoiceA: Invoice
state order: [live Invoice] = [live invoiceA]
derived total = reduce order from 0 as (sum, invoice) {
    sum + invoice.quantity
}
action broken {
    @
}
"#;

    let errors = parse_source(source).expect_err("unexpected character should be reported");
    let error = errors
        .iter()
        .find(|error| error.message.contains("unexpected character '@'"))
        .expect("expected lexer diagnostic after reduction");

    assert_eq!((error.line, error.column), (10, 5));
}
