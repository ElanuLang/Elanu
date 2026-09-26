use elanu_compiler::{
    check_source,
    runtime::{Runtime, Value},
};

fn checked_runtime(source: &str) -> Runtime {
    let program = check_source(source).expect("source should parse and check");
    Runtime::from_program(&program).expect("runtime should initialize")
}

#[test]
fn ignoring_case_matches_realistic_search_text() {
    let mut runtime = checked_runtime(
        r#"
state model LineItem {
    state description = ""
}

state model Invoice {
    state search = "bolt"
    state lines: [live LineItem] = []
    derived matches = filter lines as line {
        search == "" or line.description contains search ignoring case
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
    a.description = "Steel Bolt"
    b.description = "washer"
    invoice.lines = [live a, live b]
}
"#,
    );

    runtime.run_action("setup").unwrap();
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(1));
}

#[test]
fn ignoring_case_uses_full_unicode_case_folding() {
    let mut runtime = checked_runtime(
        r#"
state german = "Straße"
state greekUpper = "ΟΣ"
state greekFinal = "ος"

derived germanMatch = german contains "STRASSE" ignoring case
derived greekMatch = greekUpper contains greekFinal ignoring case
"#,
    );

    assert_eq!(runtime.value("germanMatch").unwrap(), Value::Bool(true));
    assert_eq!(runtime.value("greekMatch").unwrap(), Value::Bool(true));
}

#[test]
fn ignoring_case_is_locale_independent_and_does_not_normalize() {
    let mut runtime = checked_runtime(
        r#"
state asciiI = "I"
state dotlessI = "ı"
state composed = "CAFÉ"
state decomposed = "café"

derived ordinaryI = asciiI contains "i" ignoring case
derived noTurkicTailoring = asciiI contains dotlessI ignoring case
derived noNormalization = composed contains decomposed ignoring case
"#,
    );

    assert_eq!(runtime.value("ordinaryI").unwrap(), Value::Bool(true));
    assert_eq!(
        runtime.value("noTurkicTailoring").unwrap(),
        Value::Bool(false)
    );
    assert_eq!(
        runtime.value("noNormalization").unwrap(),
        Value::Bool(false)
    );
}

#[test]
fn exact_contains_remains_case_sensitive() {
    let mut runtime = checked_runtime(
        r#"
state text = "Steel Bolt"
derived exact = text contains "bolt"
derived caseless = text contains "bolt" ignoring case
"#,
    );

    assert_eq!(runtime.value("exact").unwrap(), Value::Bool(false));
    assert_eq!(runtime.value("caseless").unwrap(), Value::Bool(true));
}

#[test]
fn empty_search_short_circuits_before_caseless_text_read_then_acquires_dependency() {
    let mut runtime = checked_runtime(
        r#"
state model LineItem {
    state description = "Alpha"
}

state model Invoice {
    state search = ""
    state lines: [live LineItem] = []
    derived matches = filter lines as line {
        search == "" or line.description contains search ignoring case
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

action renameWhileEmpty {
    item.description = "Beta"
}

action startSearch {
    invoice.search = "beta"
}

action renameAfterSearch {
    item.description = "Gamma"
}
"#,
    );

    runtime.run_action("setup").unwrap();
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(1));
    let filter_name = "__elanu_filter_member$invoice$matches";
    assert_eq!(runtime.derived_evaluations(filter_name), Some(1));

    runtime.run_action("renameWhileEmpty").unwrap();
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(1));
    assert_eq!(runtime.derived_evaluations(filter_name), Some(1));

    runtime.run_action("startSearch").unwrap();
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(1));
    assert_eq!(runtime.derived_evaluations(filter_name), Some(2));

    runtime.run_action("renameAfterSearch").unwrap();
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(0));
    assert_eq!(runtime.derived_evaluations(filter_name), Some(3));
}

#[test]
fn staged_caseless_search_and_text_writes_are_visible_and_rollback_preserves_committed_world() {
    let mut runtime = checked_runtime(
        r#"
state model LineItem {
    state description = "Alpha"
}

state model Invoice {
    state search = "alpha"
    state lines: [live LineItem] = []
    derived matches = filter lines as line {
        line.description contains search ignoring case
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

action rewriteAndSearch {
    item.description = "Steel Bolt"
    invoice.search = "BOLT"
    seen = invoice.matchCount
}

action rewriteSearchThenFail {
    item.description = "Washer"
    invoice.search = "WASH"
    seen = invoice.matchCount
    fail "rollback"
}
"#,
    );

    runtime.run_action("setup").unwrap();
    runtime.run_action("rewriteAndSearch").unwrap();
    assert_eq!(runtime.value("seen").unwrap(), Value::Int(1));
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(1));

    assert!(runtime.run_action("rewriteSearchThenFail").is_err());
    assert_eq!(runtime.value("seen").unwrap(), Value::Int(1));
    assert_eq!(runtime.value("observed").unwrap(), Value::Int(1));
}

#[test]
fn ignoring_case_requires_string_operands_without_private_name_leakage() {
    for source in [
        "derived bad = 1 contains \"1\" ignoring case\n",
        "derived bad = \"1\" contains 1 ignoring case\n",
    ] {
        let errors =
            check_source(source).expect_err("caseless contains should require String operands");
        let joined = errors
            .iter()
            .map(|error| error.message.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!joined.contains("__elanu_"), "{joined}");
    }
}
