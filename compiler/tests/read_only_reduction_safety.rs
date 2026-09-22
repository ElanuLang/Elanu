use elanu_compiler::check_source;

#[test]
fn reduction_fragments_cannot_project_private_sequence_storage() {
    let errors = check_source(
        r#"
state model LineItem {
    state quantity = 1
}

state itemA: LineItem
state order: [live LineItem] = []

derived leaked = reduce order from order as (total, line) {
    total
}

action populate {
    order = [live itemA]
}
"#,
    )
    .expect_err("reduction must not expose the private sequence representation");

    assert!(errors.iter().any(|error| {
        error
            .message
            .contains("cannot project ordered sequence 'order' as an ordinary value")
    }));
}

#[test]
fn model_local_reduction_must_be_a_direct_derived_expression_in_the_first_spike() {
    let errors = check_source(
        r#"
state model LineItem {
    state quantity = 1
}

state model Invoice {
    state lines: [live LineItem] = []
    state leaked = reduce lines from 0 as (total, line) {
        total + line.quantity
    }
}

state itemA: LineItem
state invoice: Invoice
"#,
    )
    .expect_err("reduction marker must not survive as ordinary model state");

    assert!(errors.iter().any(|error| {
        error
            .message
            .contains("must be the complete expression of a derived member")
    }));
}

#[test]
fn reduce_remains_an_ordinary_identifier_outside_the_contextual_surface() {
    check_source(
        r#"
state reduce = 1
derived observed = reduce + 1
"#,
    )
    .expect("provisional reduction spelling must not reserve the identifier 'reduce'");
}
