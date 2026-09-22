use elanu_compiler::parse_source;

#[test]
fn runtime_index_expression_is_preserved_structurally() {
    let program = parse_source(
        r#"
state model LineItem {
    state quantity = 1
}
state model Invoice {
    state lines: [live LineItem] = []
}
state invoice: Invoice
state selectedLine = 0
state observed = 0

action readSelected {
    observed = invoice.lines[selectedLine + 0].quantity
}
"#,
    )
    .expect("runtime index expression should parse");

    let rendered = program.to_string();
    assert!(rendered.contains("IndexedMember invoice.lines[...].quantity"));
    assert!(rendered.contains("Binary(Add)"));
    assert!(rendered.contains("Name(selectedLine)"));
}

// The selector expression must remain structured through both writable-authority forms.
#[test]
fn runtime_index_mutation_and_authority_grant_stay_structured() {
    let program = parse_source(
        r#"
state model LineItem {
    state quantity = 1
}
state model Invoice {
    state lines: [live LineItem] = []
}
state invoice: Invoice
state selectedLine = 0

action setQuantity(state target: Int, value: Int) {
    target = value
}

action mutateSelected {
    through invoice.lines[selectedLine].quantity = 4
    setQuantity(state through invoice.lines[selectedLine].quantity, 5)
}
"#,
    )
    .expect("runtime indexed mutation and authority should parse");

    let rendered = program.to_string();
    assert!(rendered.contains("IndexedThroughAssignment invoice.lines[...].quantity Assign"));
    assert!(rendered.contains("StateGrant through invoice.lines[...].quantity"));
    assert!(rendered.matches("Name(selectedLine)").count() >= 2);
}
