from reference.experiments.identity_under_reorder import make_line_item, ordered_membership
from reference.experiments.live_state_designation import read_fact, same_target
from reference.model import Derived, State, action


def assert_targets(designations, expected):
    assert len(designations) == len(expected)
    assert all(
        same_target(designation, item)
        for designation, item in zip(designations, expected)
    )


def make_filtered_invoice_lab():
    alpha = make_line_item(sku="ALPHA", quantity=2, unit_price=5.0)
    beta = make_line_item(sku="BETA", quantity=3, unit_price=7.0)
    gamma = make_line_item(sku="GAMMA", quantity=4, unit_price=11.0)

    lines = State("lines", ordered_membership([alpha, beta, gamma]))
    search = State("search", "b")

    visible_lines = Derived(
        "visibleLines",
        lambda: [
            item
            for item in lines.get()
            if search.get().lower()
            in read_fact(item, lambda line: line.sku).lower()
        ],
    )
    visible_total = Derived(
        "visibleTotal",
        lambda: sum(
            read_fact(item, lambda line: line.line_total)
            for item in visible_lines.get()
        ),
    )

    return alpha, beta, gamma, lines, search, visible_lines, visible_total


def test_derived_structural_view_tracks_membership_and_visible_child_facts():
    alpha, beta, gamma, _lines, _search, visible_lines, visible_total = (
        make_filtered_invoice_lab()
    )

    assert_targets(visible_lines.get(), [beta])
    assert visible_total.get() == 21.0
    assert visible_lines.evaluations == 1
    assert visible_total.evaluations == 1

    # A hidden child's non-filter fact is not a dependency of either the view
    # or the downstream aggregate.
    with action():
        gamma.unit_price.set(99.0)

    assert_targets(visible_lines.get(), [beta])
    assert visible_total.get() == 21.0
    assert visible_lines.evaluations == 1
    assert visible_total.evaluations == 1

    # A visible child's aggregate fact is downstream of the view, but the view
    # itself does not depend on that fact.
    with action():
        beta.unit_price.set(8.0)

    assert_targets(visible_lines.get(), [beta])
    assert visible_total.get() == 24.0
    assert visible_lines.evaluations == 1
    assert visible_total.evaluations == 2

    # Changing a hidden child's filter key can make it enter the structural
    # view. Downstream dependencies then move to include its lineTotal.
    with action():
        gamma.sku.set("B-GAMMA")

    assert_targets(visible_lines.get(), [beta, gamma])
    assert visible_total.get() == 420.0
    assert visible_lines.evaluations == 2
    assert visible_total.evaluations == 3

    with action():
        gamma.unit_price.set(12.0)

    assert visible_total.get() == 72.0
    assert visible_total.evaluations == 4


def test_state_equivalent_derived_view_still_eagerly_invalidates_downstream():
    _alpha, beta, _gamma, _lines, _search, visible_lines, visible_total = (
        make_filtered_invoice_lab()
    )

    before = visible_lines.get()
    assert_targets(before, [beta])
    assert visible_total.get() == 21.0
    assert visible_lines.evaluations == 1
    assert visible_total.evaluations == 1

    # The filter-key state changes, so visibleLines is correctly invalidated.
    # But BETA -> BEE still produces the same structural designation sequence.
    # Current Derived.invalidate() has already invalidated visibleTotal before
    # visibleLines can recompute and discover that its value is equivalent.
    with action():
        beta.sku.set("BEE")

    after = visible_lines.get()
    assert_targets(after, [beta])
    assert before == after
    assert visible_lines.evaluations == 2

    assert visible_total.get() == 21.0
    assert visible_total.evaluations == 2


def test_filter_change_replaces_view_and_downstream_child_dependencies():
    alpha, beta, gamma, _lines, search, visible_lines, visible_total = (
        make_filtered_invoice_lab()
    )

    assert_targets(visible_lines.get(), [beta])
    assert visible_total.get() == 21.0

    with action():
        search.set("a")

    assert_targets(visible_lines.get(), [alpha, beta, gamma])
    assert visible_total.get() == 75.0

    # All three are now downstream dependencies through the recomputed view.
    with action():
        alpha.quantity.set(5)

    assert visible_total.get() == 90.0

    with action():
        search.set("b")

    assert_targets(visible_lines.get(), [beta])
    assert visible_total.get() == 21.0

    # Once Alpha leaves the view, its lineTotal dependency is removed from the
    # downstream aggregate after recomputation.
    evaluations = visible_total.evaluations
    with action():
        alpha.quantity.set(6)

    assert visible_total.get() == 21.0
    assert visible_total.evaluations == evaluations
