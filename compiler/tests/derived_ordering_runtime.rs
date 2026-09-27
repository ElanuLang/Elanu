use elanu_compiler::{
    check_source,
    runtime::{Runtime, Value},
};

fn runtime(source: &str) -> Runtime {
    let checked = check_source(source).expect("source should parse and check");
    Runtime::from_program(&checked).expect("runtime should initialize")
}

#[test]
fn failed_priority_change_rolls_back_derived_order() {
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
}

state ticketA: Ticket
state ticketB: Ticket
state ticketC: Ticket
state board: DispatchBoard

action setup {
    ticketA.priority = 3
    ticketB.priority = 2
    ticketC.priority = 1
    board.tickets = [live ticketA, live ticketB, live ticketC]
}

action raiseThenFail {
    ticketC.priority = 10
    fail "reject priority change"
}
"#,
    );

    runtime.run_action("setup").expect("setup should commit");
    let before = runtime
        .value("__elanu_filter_member$board$visibleTickets")
        .expect("ordered view should evaluate");

    let error = runtime
        .run_action("raiseThenFail")
        .expect_err("action should fail and roll back");
    assert!(error.message.contains("reject priority change"));

    assert_eq!(
        before,
        Value::Sequence {
            element_model: "Ticket".to_string(),
            targets: vec![
                "ticketA".to_string(),
                "ticketB".to_string(),
                "ticketC".to_string(),
            ],
        }
    );
    assert_eq!(
        runtime
            .value("__elanu_filter_member$board$visibleTickets")
            .expect("ordered view should retain committed order"),
        before,
        "failed ordering-key mutation must not leak a reordered derived view",
    );
}
