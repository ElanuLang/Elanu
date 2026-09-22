import pytest

from reference.experiments.identity_under_reorder import (
    make_line_item,
    ordered_membership,
)
from reference.experiments.live_state_designation import (
    designate,
    grant_member,
    read_fact,
    same_target,
)
from reference.model import Derived, State, action


def assert_membership(order, expected):
    actual = order.get()
    assert len(actual) == len(expected)
    assert all(same_target(designation, item) for designation, item in zip(actual, expected))


def test_reorder_changes_position_without_changing_logical_selection_identity():
    a = make_line_item(sku="A", quantity=2, unit_price=5.0)
    b = make_line_item(sku="B", quantity=3, unit_price=7.0)
    c = make_line_item(sku="C", quantity=4, unit_price=11.0)

    order = State("lines", ordered_membership([a, b, c]))
    selected = State("selectedLine", designate(b))

    selected_quantity = Derived(
        "selectedQuantity",
        lambda: read_fact(selected.get(), lambda item: item.quantity),
    )
    first_sku = Derived(
        "firstSku",
        lambda: read_fact(order.get()[0], lambda item: item.sku),
    )

    assert selected_quantity.get() == 3
    assert first_sku.get() == "A"
    assert selected_quantity.evaluations == 1
    assert first_sku.evaluations == 1

    with action():
        order.set(ordered_membership([c, a, b]))

    assert_membership(order, [c, a, b])
    assert same_target(selected.get(), b)

    # Logical selection does not depend on position, so reorder alone must not
    # invalidate a derived fact that reads only the selected item.
    assert selected_quantity.get() == 3
    assert selected_quantity.evaluations == 1

    # A position-based fact intentionally follows the new occupant of index 0.
    assert first_sku.get() == "C"
    assert first_sku.evaluations == 2


def test_position_based_selection_moves_to_new_logical_item_after_reorder():
    a = make_line_item(sku="A", quantity=2, unit_price=5.0)
    b = make_line_item(sku="B", quantity=3, unit_price=7.0)
    c = make_line_item(sku="C", quantity=4, unit_price=11.0)

    order = State("lines", ordered_membership([a, b, c]))
    selected_index = State("selectedIndex", 1)

    selected_by_position = Derived(
        "selectedByPosition",
        lambda: order.get()[selected_index.get()],
    )
    selected_quantity = Derived(
        "selectedQuantity",
        lambda: read_fact(selected_by_position.get(), lambda item: item.quantity),
    )

    assert selected_quantity.get() == 3
    assert selected_quantity.evaluations == 1

    with action():
        order.set(ordered_membership([b, c, a]))

    # Index 1 now denotes C, not B. This is deliberate evidence that a stable
    # logical selection cannot be represented merely by remembering an index.
    assert selected_quantity.get() == 4
    assert selected_quantity.evaluations == 2

    # Dynamic dependencies must move from B.quantity to C.quantity.
    with action():
        b.quantity.set(30)

    assert selected_quantity.get() == 4
    assert selected_quantity.evaluations == 2

    with action():
        c.quantity.set(40)

    assert selected_quantity.get() == 40
    assert selected_quantity.evaluations == 3


def test_collection_derived_recomputes_membership_but_preserves_item_dependencies():
    a = make_line_item(sku="A", quantity=2, unit_price=5.0)
    b = make_line_item(sku="B", quantity=1, unit_price=7.0)

    order = State("lines", ordered_membership([a, b]))
    subtotal = Derived(
        "subtotal",
        lambda: sum(read_fact(item, lambda line: line.line_total) for item in order.get()),
    )

    assert subtotal.get() == 17.0
    assert subtotal.evaluations == 1

    with action():
        order.set(ordered_membership([b, a]))

    # Order is a real structural dependency, so reorder invalidates subtotal
    # even though this particular aggregate's numeric result is unchanged.
    assert subtotal.get() == 17.0
    assert subtotal.evaluations == 2

    # After recomputation, member dependencies still name the logical items,
    # not their old positions.
    with action():
        b.quantity.set(3)

    assert subtotal.get() == 31.0
    assert subtotal.evaluations == 3


def test_exact_member_authority_remains_bound_to_item_across_reorder():
    a = make_line_item(sku="A", quantity=2, unit_price=5.0)
    b = make_line_item(sku="B", quantity=3, unit_price=7.0)
    c = make_line_item(sku="C", quantity=4, unit_price=11.0)

    order = State("lines", ordered_membership([a, b, c]))
    selected = State("selectedLine", designate(b))

    with action():
        authority = grant_member(selected.get(), lambda item: item.quantity)
        order.set(ordered_membership([c, a, b]))
        authority.write(9)

    assert_membership(order, [c, a, b])
    assert a.quantity.get() == 2
    assert b.quantity.get() == 9
    assert c.quantity.get() == 4


def test_transaction_local_reorder_is_visible_to_later_position_based_authority():
    a = make_line_item(sku="A", quantity=2, unit_price=5.0)
    b = make_line_item(sku="B", quantity=3, unit_price=7.0)

    order = State("lines", ordered_membership([a, b]))

    with action():
        order.set(ordered_membership([b, a]))
        first = order.get()[0]
        grant_member(first, lambda item: item.quantity).write(8)

    assert_membership(order, [b, a])
    assert a.quantity.get() == 2
    assert b.quantity.get() == 8


def test_reorder_and_item_write_roll_back_in_one_existing_transaction():
    a = make_line_item(sku="A", quantity=2, unit_price=5.0)
    b = make_line_item(sku="B", quantity=3, unit_price=7.0)

    order = State("lines", ordered_membership([a, b]))

    with pytest.raises(ValueError, match="abort"):
        with action():
            order.set(ordered_membership([b, a]))
            first = order.get()[0]
            grant_member(first, lambda item: item.quantity).write(99)
            raise ValueError("abort")

    assert_membership(order, [a, b])
    assert a.quantity.get() == 2
    assert b.quantity.get() == 3


def test_removing_membership_does_not_itself_destroy_selected_state_identity():
    a = make_line_item(sku="A", quantity=2, unit_price=5.0)
    b = make_line_item(sku="B", quantity=3, unit_price=7.0)

    order = State("lines", ordered_membership([a, b]))
    selected = State("selectedLine", designate(b))
    selected_quantity = Derived(
        "selectedQuantity",
        lambda: read_fact(selected.get(), lambda item: item.quantity),
    )

    assert selected_quantity.get() == 3
    assert selected_quantity.evaluations == 1

    with action():
        order.set(ordered_membership([a]))

    assert_membership(order, [a])
    assert same_target(selected.get(), b)

    # The current kernel has no dynamic destruction/lifetime law. Membership
    # removal therefore does not, by itself, revoke a separate live designation.
    # This is evidence to pressure future collection/lifetime design, not a
    # proposal that removed items must live forever.
    assert selected_quantity.get() == 3
    assert selected_quantity.evaluations == 1

    with action():
        grant_member(selected.get(), lambda item: item.quantity).write(6)

    assert b.quantity.get() == 6
    assert selected_quantity.get() == 6


def test_removed_item_can_be_reinserted_with_same_existing_identity_in_laboratory():
    a = make_line_item(sku="A", quantity=2, unit_price=5.0)
    b = make_line_item(sku="B", quantity=3, unit_price=7.0)

    order = State("lines", ordered_membership([a, b]))

    with action():
        order.set(ordered_membership([a]))
        b.quantity.set(10)

    with action():
        order.set(ordered_membership([b, a]))

    assert_membership(order, [b, a])
    assert read_fact(order.get()[0], lambda item: item.quantity) == 10
