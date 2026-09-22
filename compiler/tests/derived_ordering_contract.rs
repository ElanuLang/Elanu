use elanu_compiler::check_source;

// These are source-visible semantic/contract tests. They protect the accepted
// derived-ordering boundary rather than any compiler-private representation.

#[test]
fn ordered_filter_requires_explicit_direction() {
    let source = r#"
state model Ticket {
    state priority = 0
    state open = true
}

state model DispatchBoard {
    state tickets: [live Ticket] = []
    derived visibleTickets = filter tickets as ticket {
        ticket.open
    } order by ticket.priority
}

state board: DispatchBoard
"#;

    let errors = check_source(source).expect_err("ordering direction must be explicit");
    assert!(errors.iter().any(|error| error
        .message
        .contains("expected 'ascending' or 'descending' after filter ordering key")));
}

#[test]
fn ordered_filter_key_must_be_a_child_member() {
    let source = r#"
state model Ticket {
    state priority = 0
    state open = true
}

state model DispatchBoard {
    state cutoff = 0
    state tickets: [live Ticket] = []
    derived visibleTickets = filter tickets as ticket {
        ticket.open
    } order by cutoff descending
}

state board: DispatchBoard
"#;

    let errors = check_source(source).expect_err("owner ordering fact is outside the narrow delta");
    assert!(errors.iter().any(|error| error
        .message
        .contains("key must read one member through element 'ticket'")));
}

#[test]
fn ordered_filter_rejects_unknown_child_member() {
    let source = r#"
state model Ticket {
    state priority = 0
    state open = true
}

state model DispatchBoard {
    state tickets: [live Ticket] = []
    derived visibleTickets = filter tickets as ticket {
        ticket.open
    } order by ticket.missing descending
}

state board: DispatchBoard
"#;

    let errors = check_source(source).expect_err("unknown child ordering member must be rejected");
    assert!(errors.iter().any(|error| error
        .message
        .contains("state model 'Ticket' has no member 'missing'")));
}

#[test]
fn plain_filter_keeps_the_following_newline_as_a_member_separator() {
    let source = r#"
state model Ticket {
    state open = true
}

state model DispatchBoard {
    state tickets: [live Ticket] = []
    derived visibleTickets = filter tickets as ticket {
        ticket.open
    }
    state marker = 1
}

state board: DispatchBoard
"#;

    check_source(source).expect("plain filter must not consume the next member separator");
}
