from types import SimpleNamespace

import pytest

from reference.experiments.derived_stabilization import StabilizingDerived
from reference.experiments.live_state_designation import designate, read_fact, same_target
from reference.model import DerivedCycle, State, action


def make_item(*, sku: str, quantity: int, unit_price: float):
    item = SimpleNamespace()
    item.sku = State(f"{sku}.sku", sku)
    item.quantity = State(f"{sku}.quantity", quantity)
    item.unit_price = State(f"{sku}.unitPrice", unit_price)
    item.line_total = StabilizingDerived(
        f"{sku}.lineTotal",
        lambda: item.quantity.get() * item.unit_price.get(),
    )
    return item


def assert_targets(designations, expected):
    assert len(designations) == len(expected)
    assert all(
        same_target(designation, item)
        for designation, item in zip(designations, expected)
    )


def make_filtered_invoice_lab():
    alpha = make_item(sku="ALPHA", quantity=2, unit_price=5.0)
    beta = make_item(sku="BETA", quantity=3, unit_price=7.0)
    gamma = make_item(sku="GAMMA", quantity=4, unit_price=11.0)

    lines = State("lines", [designate(alpha), designate(beta), designate(gamma)])
    search = State("search", "b")

    visible_lines = StabilizingDerived(
        "visibleLines",
        lambda: [
            item
            for item in lines.get()
            if search.get().lower()
            in read_fact(item, lambda line: line.sku).lower()
        ],
    )
    visible_total = StabilizingDerived(
        "visibleTotal",
        lambda: sum(
            read_fact(item, lambda line: line.line_total)
            for item in visible_lines.get()
        ),
    )

    return alpha, beta, gamma, lines, search, visible_lines, visible_total


def test_equivalent_structural_view_stabilizes_before_downstream_recompute():
    _alpha, beta, _gamma, _lines, _search, visible_lines, visible_total = (
        make_filtered_invoice_lab()
    )

    before = visible_lines.get()
    assert_targets(before, [beta])
    assert visible_total.get() == 21.0
    assert (visible_lines.evaluations, visible_total.evaluations) == (1, 1)

    with action():
        beta.sku.set("BEE")

    assert visible_lines.dirty
    assert visible_total.dirty

    # Reading the downstream value pulls revalidation through visibleLines.
    # visibleLines really recomputes, but its structural designation sequence is
    # equivalent, so visibleTotal keeps its cache without running again.
    assert visible_total.get() == 21.0
    after = visible_lines.get()
    assert_targets(after, [beta])
    assert before == after
    assert (visible_lines.evaluations, visible_total.evaluations) == (2, 1)
    assert not visible_lines.dirty
    assert not visible_total.dirty


def test_true_structural_view_change_still_recomputes_downstream():
    _alpha, beta, gamma, _lines, _search, visible_lines, visible_total = (
        make_filtered_invoice_lab()
    )

    assert_targets(visible_lines.get(), [beta])
    assert visible_total.get() == 21.0

    with action():
        gamma.sku.set("B-GAMMA")

    assert visible_total.get() == 65.0
    assert_targets(visible_lines.get(), [beta, gamma])
    assert (visible_lines.evaluations, visible_total.evaluations) == (2, 2)


def test_stable_recompute_can_replace_dynamic_dependencies():
    choose_a = State("chooseA", True)
    a = State("a", 5)
    b = State("b", 5)
    selected = StabilizingDerived(
        "selected", lambda: a.get() if choose_a.get() else b.get()
    )
    doubled = StabilizingDerived("doubled", lambda: selected.get() * 2)

    assert doubled.get() == 10
    assert a in selected.dependencies
    assert b not in selected.dependencies

    # The selected value remains 5, but the dependency topology changes from a
    # to b. The downstream result therefore stays cached while selected installs
    # its new dependency set.
    with action():
        choose_a.set(False)

    assert doubled.get() == 10
    assert (selected.evaluations, doubled.evaluations) == (2, 1)
    assert a not in selected.dependencies
    assert b in selected.dependencies

    # The old branch is now detached.
    with action():
        a.set(6)
    assert doubled.get() == 10
    assert (selected.evaluations, doubled.evaluations) == (2, 1)

    # The new branch remains live and a real value change propagates normally.
    with action():
        b.set(6)
    assert doubled.get() == 12
    assert (selected.evaluations, doubled.evaluations) == (3, 2)


def test_one_changed_diamond_input_recomputes_downstream_once():
    source = State("source", 1)
    stable = StabilizingDerived("stable", lambda: source.get() % 2)
    changed = StabilizingDerived("changed", lambda: source.get() * 10)
    combined = StabilizingDerived("combined", lambda: stable.get() + changed.get())

    assert combined.get() == 11
    assert (stable.evaluations, changed.evaluations, combined.evaluations) == (1, 1, 1)

    with action():
        source.set(3)

    # combined has two suspect upstream causes, so the conservative candidate
    # reevaluates combined normally instead of speculatively walking the old
    # dependency graph. Each upstream still refreshes correctly when read.
    assert combined.get() == 31
    assert (stable.evaluations, changed.evaluations, combined.evaluations) == (2, 2, 2)


def test_failed_upstream_revalidation_never_serves_stale_downstream_cache():
    ready = State("ready", True)
    source = State("source", 1)

    def compute_upstream():
        if not ready.get():
            raise ValueError("not ready")
        return source.get()

    upstream = StabilizingDerived("upstream", compute_upstream)
    downstream = StabilizingDerived("downstream", lambda: upstream.get() * 2)

    assert downstream.get() == 2

    with action():
        ready.set(False)

    with pytest.raises(ValueError, match="not ready"):
        downstream.get()

    # The old downstream cache exists internally but cannot be observed while
    # its upstream value cannot be revalidated.
    assert downstream.evaluations == 1
    assert downstream.dirty

    with action():
        ready.set(True)

    # The upstream returns to the same observable value, so downstream may now
    # safely reuse its original cache.
    assert downstream.get() == 2
    assert upstream.evaluations == 2
    assert downstream.evaluations == 1


def test_multiple_suspect_causes_fall_back_to_normal_control_flow():
    choose_risky = State("chooseRisky", True)
    risky_ready = State("riskyReady", True)

    selector = StabilizingDerived("selector", lambda: choose_risky.get())

    def compute_risky():
        if not risky_ready.get():
            raise ValueError("risky branch unavailable")
        return 10

    risky = StabilizingDerived("risky", compute_risky)
    safe = StabilizingDerived("safe", lambda: 20)
    output = StabilizingDerived(
        "output", lambda: risky.get() if selector.get() else safe.get()
    )

    assert output.get() == 10
    assert risky.evaluations == 1

    # Both old upstream values become suspect. The conservative candidate does
    # not choose an order in which to revalidate that old graph. It reevaluates
    # output normally; current control flow reads selector, chooses safe, and
    # never evaluates the obsolete now-failing risky dependency.
    with action():
        choose_risky.set(False)
        risky_ready.set(False)

    assert output.get() == 20
    assert output.evaluations == 2
    assert risky.evaluations == 1
    assert risky.dirty


def test_dynamic_cycle_discovered_during_revalidation_fails_instead_of_using_cache():
    cyclic = State("cyclic", False)
    holder = {}

    a = StabilizingDerived(
        "a", lambda: holder["b"].get() + 1 if cyclic.get() else 1
    )
    b = StabilizingDerived("b", lambda: a.get() + 1)
    holder["b"] = b

    assert b.get() == 2

    with action():
        cyclic.set(True)

    with pytest.raises(DerivedCycle, match="b -> a -> b"):
        b.get()


def test_transaction_local_dependencies_do_not_replace_committed_graph_on_rollback():
    choose_a = State("chooseA", True)
    a = State("a", 10)
    b = State("b", 20)
    selected = StabilizingDerived(
        "selected", lambda: a.get() if choose_a.get() else b.get()
    )

    assert selected.get() == 10
    assert a in selected.dependencies
    assert b not in selected.dependencies

    with pytest.raises(ValueError, match="rollback"):
        with action():
            choose_a.set(False)
            assert selected.get() == 20
            # Transaction-local dependency tracking is separate; the committed
            # graph must still describe the last committed evaluation.
            assert a in selected.dependencies
            assert b not in selected.dependencies
            raise ValueError("rollback")

    assert choose_a.get() is True
    assert selected.get() == 10
    assert a in selected.dependencies
    assert b not in selected.dependencies


def test_transaction_local_cache_still_observes_later_staged_writes():
    source = State("source", 2)
    doubled = StabilizingDerived("doubled", lambda: source.get() * 2)

    with action():
        assert doubled.get() == 4
        before = doubled.evaluations
        source.set(5)
        assert doubled.get() == 10
        assert doubled.evaluations == before + 1
