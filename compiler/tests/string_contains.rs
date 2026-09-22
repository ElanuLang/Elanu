use elanu_compiler::{
    check_source,
    runtime::{Runtime, Value},
};

fn checked_runtime(source: &str) -> Runtime {
    let program = check_source(source).expect("source should parse and check");
    Runtime::from_program(&program).expect("runtime should initialize")
}

#[test]
fn contains_filters_child_text_by_owner_search_text() {
    let mut runtime = checked_runtime(
        r#"
state model LineItem {
    state description = ""
}

state model Invoice {
    state search = "bolt"
    state lines: [live LineItem] = []
    derived matches = filter lines as line {
        line.description contains search
    }
    derived matchCount = reduce matches from 0 as (count, line) {
        count + 1
    }
}

state a: LineItem
state b: LineItem
state invoice: Invoice

derived observed = invoice.matchCount

action setup {
    a.description = "steel bolt"
    b.description = "washer"
    invoice.lines = [live a, live b]
}
"#,
    );

    runtime.run_action("setup").unwrap();
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(1));
}

#[test]
fn empty_search_is_contained_in_every_string() {
    let mut runtime = checked_runtime(
        r#"
state model LineItem {
    state description = ""
}

state model Invoice {
    state search = ""
    state lines: [live LineItem] = []
    derived matches = filter lines as line {
        line.description contains search
    }
    derived matchCount = reduce matches from 0 as (count, line) {
        count + 1
    }
}

state a: LineItem
state b: LineItem
state invoice: Invoice

derived observed = invoice.matchCount

action setup {
    a.description = "anything"
    b.description = ""
    invoice.lines = [live a, live b]
}
"#,
    );

    runtime.run_action("setup").unwrap();
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(2));
}

#[test]
fn contains_is_case_sensitive_and_does_not_normalize_unicode() {
    let mut runtime = checked_runtime(
        r#"
state upper = "Alpha"
state lower = "alpha"
state composed = "café"
state decomposed = "café"

derived sameCase = upper contains "Alpha"
derived differentCase = upper contains lower
derived composedMatch = composed contains "fé"
derived normalizedForMe = composed contains decomposed
"#,
    );

    assert_eq!(runtime.value("sameCase").unwrap(), Value::Bool(true));
    assert_eq!(runtime.value("differentCase").unwrap(), Value::Bool(false));
    assert_eq!(runtime.value("composedMatch").unwrap(), Value::Bool(true));
    assert_eq!(
        runtime.value("normalizedForMe").unwrap(),
        Value::Bool(false)
    );
}

#[test]
fn empty_search_short_circuit_avoids_child_text_dependencies() {
    let mut runtime = checked_runtime(
        r#"
state model LineItem {
    state description = "alpha"
}

state model Invoice {
    state search = ""
    state lines: [live LineItem] = []
    derived matches = filter lines as line {
        search == "" or line.description contains search
    }
    derived matchCount = reduce matches from 0 as (count, line) {
        count + 1
    }
}

state item: LineItem
state invoice: Invoice

derived observed = invoice.matchCount

action setup {
    invoice.lines = [live item]
}

action rename {
    item.description = "renamed"
}

action renameAgain {
    item.description = "name changed"
}

action startSearch {
    invoice.search = "name"
}
"#,
    );

    runtime.run_action("setup").unwrap();
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(1));

    let filter_name = "__meld_filter_member$invoice$matches";
    assert_eq!(runtime.derived_evaluations(filter_name), Some(1));

    runtime.run_action("rename").unwrap();
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(1));
    assert_eq!(runtime.derived_evaluations(filter_name), Some(1));

    runtime.run_action("startSearch").unwrap();
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(1));
    assert_eq!(runtime.derived_evaluations(filter_name), Some(2));

    runtime.run_action("renameAgain").unwrap();
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(1));
    assert_eq!(runtime.derived_evaluations(filter_name), Some(3));
}

#[test]
fn staged_search_and_text_writes_are_visible_to_contains() {
    let mut runtime = checked_runtime(
        r#"
state model LineItem {
    state description = "old"
}

state model Invoice {
    state search = "old"
    state lines: [live LineItem] = []
    derived matches = filter lines as line {
        line.description contains search
    }
    derived matchCount = reduce matches from 0 as (count, line) {
        count + 1
    }
}

state item: LineItem
state invoice: Invoice
state seen = 0

action setup {
    invoice.lines = [live item]
}

action rewriteAndSearch {
    item.description = "new description"
    invoice.search = "new"
    seen = invoice.matchCount
}
"#,
    );

    runtime.run_action("setup").unwrap();
    runtime.run_action("rewriteAndSearch").unwrap();
    assert_eq!(runtime.value("seen").unwrap(), Value::Int(1));
}

#[test]
fn rollback_discards_temporary_contains_result_and_dependencies() {
    let mut runtime = checked_runtime(
        r#"
state model LineItem {
    state description = "alpha"
}

state model Invoice {
    state search = ""
    state lines: [live LineItem] = []
    derived matches = filter lines as line {
        search == "" or line.description contains search
    }
    derived matchCount = reduce matches from 0 as (count, line) {
        count + 1
    }
}

state item: LineItem
state invoice: Invoice
state seen = 0

derived observed = invoice.matchCount

action setup {
    invoice.lines = [live item]
}

action searchThenFail {
    invoice.search = "zzz"
    seen = invoice.matchCount
    fail "rollback"
}

action rename {
    item.description = "renamed"
}
"#,
    );

    runtime.run_action("setup").unwrap();
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(1));
    let filter_name = "__meld_filter_member$invoice$matches";
    assert_eq!(runtime.derived_evaluations(filter_name), Some(1));

    assert!(runtime.run_action("searchThenFail").is_err());
    assert_eq!(runtime.value("seen").unwrap(), Value::Int(0));
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(1));

    // Evaluation counters are instrumentation, not transactional language state, so
    // the failed transaction may increment this count. What must roll back is the
    // temporary dependency on item.description.
    let evaluations_after_failure = runtime.derived_evaluations(filter_name);
    assert_eq!(evaluations_after_failure, Some(2));

    runtime.run_action("rename").unwrap();
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(1));
    assert_eq!(
        runtime.derived_evaluations(filter_name),
        evaluations_after_failure
    );
}

#[test]
fn contains_requires_string_operands() {
    for source in [
        "derived bad = 1 contains \"1\"\n",
        "derived bad = \"1\" contains 1\n",
    ] {
        let errors = check_source(source).expect_err("contains should require String operands");
        let joined = errors
            .iter()
            .map(|error| error.message.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            joined.contains("contains") || joined.contains("Contains"),
            "{joined}"
        );
        assert!(!joined.contains("__meld_"), "{joined}");
    }
}

#[test]
fn contains_binds_more_tightly_than_boolean_composition() {
    let mut runtime = checked_runtime(
        r#"
state text = "alphabet"
state needle = "pha"
state enabled = true

derived result = text contains needle and enabled
"#,
    );

    assert_eq!(runtime.value("result").unwrap(), Value::Bool(true));
}
