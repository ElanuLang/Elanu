use elanu_compiler::{
    check_source,
    runtime::{Runtime, Value},
};

#[test]
fn owner_relative_conditional_reduction_validation_does_not_unroll_per_element() {
    // The pre-fix validation oracle expanded a conditional accumulator roughly
    // as 2^N. COUNT=100 was already unable to complete within 45 seconds in an
    // optimized measurement run, so this is a practical regression guard
    // without relying on a wall-clock assertion.
    const COUNT: usize = 100;

    let mut source = String::from(
        r#"
state model LineItem {
    state quantity = 1
}

state model Invoice {
    state minimum = 2
    state lines: [live LineItem] = []
    derived qualifyingTotal = reduce lines from 0 as (total, line) {
        if line.quantity >= minimum {
            total + line.quantity
        } else {
            total
        }
    }
}

"#,
    );

    for index in 0..COUNT {
        source.push_str(&format!("state item{index}: LineItem\n"));
    }
    source.push_str(
        "state invoice: Invoice\n\nderived observed = invoice.qualifyingTotal\n\naction populate {\n    invoice.lines = [",
    );
    for index in 0..COUNT {
        if index > 0 {
            source.push_str(", ");
        }
        source.push_str(&format!("live item{index}"));
    }
    source.push_str("]\n}\n\naction includeAll {\n    invoice.minimum = 1\n}\n");

    let checked = check_source(&source).expect("100-item conditional reduction should check");
    let mut runtime = Runtime::from_program(&checked).expect("runtime should initialize");
    runtime.run_action("populate").unwrap();
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(0));

    runtime.run_action("includeAll").unwrap();
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(COUNT as i64));
}
