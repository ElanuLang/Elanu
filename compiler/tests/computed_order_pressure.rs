use elanu_compiler::{
    check_source,
    runtime::{Runtime, Value},
};

fn runtime(source: &str) -> Runtime {
    let checked = check_source(source).expect("source should parse and check");
    Runtime::from_program(&checked).expect("runtime should initialize")
}

#[test]
fn priority_change_requires_explicit_structural_reorder_to_change_display_order() {
    let mut runtime = runtime(
        r#"
state model Ticket {
    state priority = 0
    state open = true
}

state model DispatchBoard {
    state tickets: [live Ticket] = []

    derived visibleTickets = filter tickets as ticket {
        ticket.open
    }
}

state ticketA: Ticket
state ticketB: Ticket
state ticketC: Ticket
state board: DispatchBoard
state selected: live Ticket = live ticketC
state anchor: live Ticket = live ticketA

action setup {
    ticketA.priority = 1
    ticketB.priority = 2
    ticketC.priority = 3
    board.tickets = [live ticketA, live ticketB, live ticketC]
}

action raiseSelectedPriority {
    ticketC.priority = 10
}

action manuallySynchronizeDisplayOrder {
    move selected before anchor in board.visibleTickets
}
"#,
    );

    runtime.run_action("setup").expect("setup should commit");

    assert_eq!(
        runtime
            .value("__meld_filter_member$board$visibleTickets")
            .unwrap(),
        Value::Sequence {
            element_model: "Ticket".to_string(),
            targets: vec![
                "ticketA".to_string(),
                "ticketB".to_string(),
                "ticketC".to_string(),
            ],
        }
    );

    runtime
        .run_action("raiseSelectedPriority")
        .expect("priority change should commit");

    assert_eq!(
        runtime
            .value("__meld_filter_member$board$visibleTickets")
            .unwrap(),
        Value::Sequence {
            element_model: "Ticket".to_string(),
            targets: vec![
                "ticketA".to_string(),
                "ticketB".to_string(),
                "ticketC".to_string(),
            ],
        },
        "filter membership reacts to predicate facts but preserves backing membership order",
    );

    runtime
        .run_action("manuallySynchronizeDisplayOrder")
        .expect("explicit structural move should commit");

    assert_eq!(
        runtime
            .value("__meld_filter_member$board$visibleTickets")
            .unwrap(),
        Value::Sequence {
            element_model: "Ticket".to_string(),
            targets: vec![
                "ticketC".to_string(),
                "ticketA".to_string(),
                "ticketB".to_string(),
            ],
        },
        "current Elanu requires an explicit structural edit to make display order follow priority",
    );
}

#[test]
fn ordered_filter_tracks_child_priority_without_mutating_backing_membership() {
    let mut runtime = runtime(
        r#"
state model Ticket {
    state priority = 0
    state open = true
}

state model DispatchBoard {
    state tickets: [live Ticket] = []

    derived visibleTickets = filter tickets as ticket {
        ticket.open
    } order by ticket.priority descending

    derived ascendingTickets = filter tickets as ticket {
        ticket.open
    } order by ticket.priority ascending
}

state ticketA: Ticket
state ticketB: Ticket
state ticketC: Ticket
state board: DispatchBoard

action setup {
    ticketA.priority = 1
    ticketB.priority = 3
    ticketC.priority = 3
    board.tickets = [live ticketA, live ticketB, live ticketC]
}

action raiseC {
    ticketC.priority = 5
}

action closeB {
    ticketB.open = false
}
"#,
    );

    runtime.run_action("setup").expect("setup should commit");

    assert_eq!(
        runtime
            .value("__meld_filter_member$board$visibleTickets")
            .unwrap(),
        Value::Sequence {
            element_model: "Ticket".to_string(),
            targets: vec![
                "ticketB".to_string(),
                "ticketC".to_string(),
                "ticketA".to_string(),
            ],
        },
        "equal priority keys retain source order",
    );
    assert_eq!(
        runtime
            .value("__meld_filter_member$board$ascendingTickets")
            .unwrap(),
        Value::Sequence {
            element_model: "Ticket".to_string(),
            targets: vec![
                "ticketA".to_string(),
                "ticketB".to_string(),
                "ticketC".to_string(),
            ],
        },
    );
    assert_eq!(
        runtime.value("__meld_mseq$board$tickets").unwrap(),
        Value::Sequence {
            element_model: "Ticket".to_string(),
            targets: vec![
                "ticketA".to_string(),
                "ticketB".to_string(),
                "ticketC".to_string(),
            ],
        },
        "derived ordering must not mutate backing membership",
    );

    runtime
        .run_action("raiseC")
        .expect("priority change should commit");

    assert_eq!(
        runtime
            .value("__meld_filter_member$board$visibleTickets")
            .unwrap(),
        Value::Sequence {
            element_model: "Ticket".to_string(),
            targets: vec![
                "ticketC".to_string(),
                "ticketB".to_string(),
                "ticketA".to_string(),
            ],
        },
        "ordered view must react to current child ordering facts",
    );
    assert_eq!(
        runtime.value("__meld_mseq$board$tickets").unwrap(),
        Value::Sequence {
            element_model: "Ticket".to_string(),
            targets: vec![
                "ticketA".to_string(),
                "ticketB".to_string(),
                "ticketC".to_string(),
            ],
        },
        "priority-driven view changes must leave structural order untouched",
    );

    runtime
        .run_action("closeB")
        .expect("membership fact should commit");
    assert_eq!(
        runtime
            .value("__meld_filter_member$board$visibleTickets")
            .unwrap(),
        Value::Sequence {
            element_model: "Ticket".to_string(),
            targets: vec!["ticketC".to_string(), "ticketA".to_string()],
        },
        "filter membership still controls which identities participate before ordering",
    );
}

#[test]
fn ordered_filter_rejects_non_int_keys() {
    let source = r#"
state model Ticket {
    state open = true
}

state model DispatchBoard {
    state tickets: [live Ticket] = []
    derived visibleTickets = filter tickets as ticket {
        ticket.open
    } order by ticket.open descending
}

state board: DispatchBoard
"#;

    let errors = check_source(source).expect_err("Bool ordering key must be rejected");
    assert!(errors
        .iter()
        .any(|error| error.message.contains("key 'ticket.open' must be Int")));
}

#[test]
fn ordered_filter_is_not_a_structural_remove_selector() {
    let source = r#"
state model Ticket {
    state priority = 0
    state open = true
}

state model DispatchBoard {
    state tickets: [live Ticket] = []
    derived visibleTickets = filter tickets as ticket {
        ticket.open
    } order by ticket.priority descending
}

state ticketA: Ticket
state board: DispatchBoard
state selected: live Ticket = live ticketA

action removeFromOrderedView {
    remove selected from board.visibleTickets
}
"#;

    let errors = check_source(source).expect_err("ordered view removal selector must be rejected");
    assert!(errors.iter().any(|error| error
        .message
        .contains("cannot be a structural removal target")));
}

#[test]
fn ordered_filter_is_not_a_structural_move_selector() {
    let source = r#"
state model Ticket {
    state priority = 0
    state open = true
}

state model DispatchBoard {
    state tickets: [live Ticket] = []
    derived visibleTickets = filter tickets as ticket {
        ticket.open
    } order by ticket.priority descending
}

state ticketA: Ticket
state ticketB: Ticket
state board: DispatchBoard
state selected: live Ticket = live ticketA
state anchor: live Ticket = live ticketB

action moveWithinOrderedView {
    move selected before anchor in board.visibleTickets
}
"#;

    let errors = check_source(source).expect_err("ordered view move selector must be rejected");
    assert!(errors
        .iter()
        .any(|error| error.message.contains("cannot be a structural move target")));
}
