use elanu_compiler::{
    ast::Expr,
    parse_source,
    runtime_model_templates::{collect, RuntimeModelMemberKind},
    semantic::ValueType,
};

#[test]
fn collects_typed_state_model_members_without_flattening_them() {
    let program = parse_source(
        r#"
state model LineItem {
    state quantity = 1
    state unitPrice = 10.0
    derived lineTotal = quantity * unitPrice
}

state model Invoice {
    state lines: [live LineItem] = []
}

state invoice: Invoice
"#,
    )
    .expect("source should parse");

    let templates = collect(&program.state_models).expect("model templates should resolve");

    let line_item = templates.get("LineItem").expect("LineItem template");
    assert_eq!(line_item.members.len(), 3);

    let quantity = line_item.member("quantity").expect("quantity member");
    assert_eq!(quantity.kind, RuntimeModelMemberKind::State);
    assert_eq!(quantity.value_type, ValueType::Int);
    assert_eq!(quantity.expression, Expr::Integer(1));

    let unit_price = line_item.member("unitPrice").expect("unitPrice member");
    assert_eq!(unit_price.kind, RuntimeModelMemberKind::State);
    assert_eq!(unit_price.value_type, ValueType::Float);
    assert_eq!(unit_price.expression, Expr::Float(10.0));

    let line_total = line_item.member("lineTotal").expect("lineTotal member");
    assert_eq!(line_total.kind, RuntimeModelMemberKind::Derived);
    assert_eq!(line_total.value_type, ValueType::Float);
    assert!(matches!(
        &line_total.expression,
        Expr::Binary { left, right, .. }
            if matches!(left.as_ref(), Expr::Name(name) if name == "quantity")
                && matches!(right.as_ref(), Expr::Name(name) if name == "unitPrice")
    ));

    let invoice = templates.get("Invoice").expect("Invoice template");
    let lines = invoice.member("lines").expect("lines member");
    assert_eq!(lines.kind, RuntimeModelMemberKind::State);
    assert_eq!(
        lines.value_type,
        ValueType::SequenceLive("LineItem".to_string())
    );
}

#[test]
fn template_collection_reuses_model_type_validation() {
    let program = parse_source(
        r#"
state model Broken {
    state quantity: Bool = 1
}
"#,
    )
    .expect("source should parse before model typing");

    let diagnostics = collect(&program.state_models).expect_err("invalid model should be rejected");
    assert!(diagnostics
        .iter()
        .any(|diagnostic| diagnostic.message.contains("declared as Bool")));
}
