use std::alloc::{GlobalAlloc, Layout, System};
use std::env;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use elanu_compiler::{
    check_source,
    runtime::{Runtime, Value},
};

struct CountingAllocator;

static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);
static ALLOCATED_BYTES: AtomicU64 = AtomicU64::new(0);

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        ALLOCATED_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        ALLOCATED_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        ALLOCATED_BYTES.fetch_add(new_size as u64, Ordering::Relaxed);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: CountingAllocator = CountingAllocator;

#[derive(Debug, Clone, Copy)]
struct Measurement {
    elapsed: Duration,
    allocations: u64,
    allocated_bytes: u64,
}

fn measure<T>(operation: impl FnOnce() -> T) -> (T, Measurement) {
    ALLOCATIONS.store(0, Ordering::Relaxed);
    ALLOCATED_BYTES.store(0, Ordering::Relaxed);
    let started = Instant::now();
    let result = operation();
    let elapsed = started.elapsed();
    let allocations = ALLOCATIONS.load(Ordering::Relaxed);
    let allocated_bytes = ALLOCATED_BYTES.load(Ordering::Relaxed);
    (
        result,
        Measurement {
            elapsed,
            allocations,
            allocated_bytes,
        },
    )
}

fn generate_source(count: usize) -> String {
    let middle = count / 2;
    let mut source = String::new();
    source.push_str(
        r#"
state model LineItem {
    state quantity = 3
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

    for index in 0..count {
        source.push_str(&format!("state item{index}: LineItem\n"));
    }

    source.push_str(
        r#"
state invoice: Invoice

derived observed = invoice.qualifyingTotal

action populate {
    invoice.lines = [
"#,
    );
    for index in 0..count {
        source.push_str(&format!("        live item{index}"));
        if index + 1 != count {
            source.push(',');
        }
        source.push('\n');
    }
    source.push_str(
        r#"    ]
}

action sameAgain {
    invoice.lines = [
"#,
    );
    for index in 0..count {
        source.push_str(&format!("        live item{index}"));
        if index + 1 != count {
            source.push(',');
        }
        source.push('\n');
    }
    source.push_str(&format!(
        r#"    ]
}}

action changeMiddle {{
    item{middle}.quantity = 5
}}

action raiseMinimum {{
    invoice.minimum = 4
}}

action lowerMinimum {{
    invoice.minimum = 2
}}
"#,
    ));
    source
}

fn print_measurement(name: &str, measurement: Measurement) {
    println!(
        "phase={name} elapsed_us={} allocations={} allocated_bytes={}",
        measurement.elapsed.as_micros(),
        measurement.allocations,
        measurement.allocated_bytes
    );
}

fn main() {
    let count = env::args()
        .nth(1)
        .unwrap_or_else(|| "1000".to_string())
        .parse::<usize>()
        .expect("item count must be a positive integer");
    assert!(count > 0, "item count must be positive");

    println!("experiment=E item_count={count}");

    let (source, source_measurement) = measure(|| generate_source(count));
    print_measurement("generate_source", source_measurement);
    println!("source_bytes={}", source.len());

    let (checked, check_measurement) = measure(|| check_source(&source));
    print_measurement("check_source", check_measurement);
    let checked = checked.expect("generated Experiment E source should check");

    let (runtime, runtime_init_measurement) = measure(|| Runtime::from_program(&checked));
    print_measurement("runtime_init", runtime_init_measurement);
    let mut runtime = runtime.expect("runtime should initialize");

    let (_, populate_measurement) = measure(|| runtime.run_action("populate").unwrap());
    print_measurement("populate", populate_measurement);

    let (first, first_measurement) = measure(|| runtime.value("observed").unwrap());
    print_measurement("first_aggregate", first_measurement);
    assert_eq!(first, Value::Int((count as i64) * 3));
    println!(
        "first_observed_evaluations={}",
        runtime.derived_evaluations("observed").unwrap_or(0)
    );

    let (_, child_write_measurement) = measure(|| runtime.run_action("changeMiddle").unwrap());
    print_measurement("child_write", child_write_measurement);
    let (after_child, child_recompute_measurement) = measure(|| runtime.value("observed").unwrap());
    print_measurement("child_recompute", child_recompute_measurement);
    assert_eq!(after_child, Value::Int((count as i64) * 3 + 2));
    println!(
        "after_child_observed_evaluations={}",
        runtime.derived_evaluations("observed").unwrap_or(0)
    );

    let (_, filter_write_measurement) = measure(|| runtime.run_action("raiseMinimum").unwrap());
    print_measurement("filter_write", filter_write_measurement);
    let (after_filter, filter_recompute_measurement) =
        measure(|| runtime.value("observed").unwrap());
    print_measurement("filter_recompute", filter_recompute_measurement);
    assert_eq!(after_filter, Value::Int(5));
    println!(
        "after_filter_observed_evaluations={}",
        runtime.derived_evaluations("observed").unwrap_or(0)
    );

    let before_noop_evaluations = runtime.derived_evaluations("observed").unwrap_or(0);
    let (_, noop_measurement) = measure(|| runtime.run_action("sameAgain").unwrap());
    print_measurement("same_sequence_write", noop_measurement);
    let (after_noop, noop_read_measurement) = measure(|| runtime.value("observed").unwrap());
    print_measurement("read_after_same_sequence", noop_read_measurement);
    assert_eq!(after_noop, Value::Int(5));
    assert_eq!(
        runtime.derived_evaluations("observed").unwrap_or(0),
        before_noop_evaluations,
        "state-equivalent sequence replacement must not invalidate the aggregate"
    );

    let (_, lower_filter_measurement) = measure(|| runtime.run_action("lowerMinimum").unwrap());
    print_measurement("lower_filter_write", lower_filter_measurement);
    let (restored, restored_measurement) = measure(|| runtime.value("observed").unwrap());
    print_measurement("restored_filter_recompute", restored_measurement);
    assert_eq!(restored, Value::Int((count as i64) * 3 + 2));
    println!(
        "final_observed_evaluations={}",
        runtime.derived_evaluations("observed").unwrap_or(0)
    );
}
