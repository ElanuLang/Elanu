import pytest

from reference.experiments.dynamic_state_lifetime import (
    attach,
    contains,
    create_line,
    detach,
    drop_owner_record_for_experiment,
    make_dynamic_line_scope,
    owns,
)
from reference.experiments.live_state_designation import grant_member, read_fact
from reference.model import Derived, State, action


def test_dynamic_creation_commits_scope_ownership_and_membership_atomically():
    scope = make_dynamic_line_scope()

    with action():
        line = create_line(
            scope,
            sku="A",
            quantity=2,
            unit_price=5.0,
        )
        grant_member(line, lambda item: item.quantity).write(3)

        assert owns(scope, line)
        assert contains(scope, line)
        assert read_fact(line, lambda item: item.quantity) == 3

    assert owns(scope, line)
    assert contains(scope, line)
    assert read_fact(line, lambda item: item.quantity) == 3


def test_failed_creation_leaves_no_committed_scope_registration():
    scope = make_dynamic_line_scope()
    escaped_host_reference = []

    with pytest.raises(ValueError, match="abort"):
        with action():
            line = create_line(
                scope,
                sku="A",
                quantity=2,
                unit_price=5.0,
            )
            escaped_host_reference.append(line)
            assert owns(scope, line)
            assert contains(scope, line)
            raise ValueError("abort")

    assert scope.owned.get() == []
    assert scope.order.get() == []

    # The host-language object may still physically exist because the Python
    # test deliberately retained it. That is not committed Meld semantic
    # lifetime. The scope refuses to attach it because its ownership
    # registration rolled back.
    line = escaped_host_reference[0]
    with pytest.raises(ValueError, match="not owned"):
        with action():
            attach(scope, line)


def test_detach_preserves_scope_owned_identity_and_independent_selection():
    scope = make_dynamic_line_scope()

    with action():
        line = create_line(
            scope,
            sku="A",
            quantity=2,
            unit_price=5.0,
        )

    selected = State("selectedLine", line)
    selected_quantity = Derived(
        "selectedQuantity",
        lambda: read_fact(selected.get(), lambda item: item.quantity),
    )

    assert selected_quantity.get() == 2

    with action():
        detach(scope, line)

    assert owns(scope, line)
    assert not contains(scope, line)
    assert selected.get() == line
    assert selected_quantity.get() == 2

    with action():
        grant_member(selected.get(), lambda item: item.quantity).write(7)

    assert selected_quantity.get() == 7


def test_reattach_preserves_same_identity_and_state():
    scope = make_dynamic_line_scope()

    with action():
        line = create_line(
            scope,
            sku="A",
            quantity=2,
            unit_price=5.0,
        )
        grant_member(line, lambda item: item.quantity).write(9)

    with action():
        detach(scope, line)

    with action():
        attach(scope, line)

    assert owns(scope, line)
    assert contains(scope, line)
    assert read_fact(scope.order.get()[0], lambda item: item.quantity) == 9
    assert scope.order.get()[0] == line


def test_visible_aggregate_excludes_detached_item_while_selection_keeps_following_it():
    scope = make_dynamic_line_scope()

    with action():
        a = create_line(scope, sku="A", quantity=2, unit_price=5.0)
        b = create_line(scope, sku="B", quantity=3, unit_price=7.0)

    selected = State("selectedLine", b)
    subtotal = Derived(
        "subtotal",
        lambda: sum(
            read_fact(line, lambda item: item.line_total)
            for line in scope.order.get()
        ),
    )
    selected_quantity = Derived(
        "selectedQuantity",
        lambda: read_fact(selected.get(), lambda item: item.quantity),
    )

    assert subtotal.get() == 31.0
    assert selected_quantity.get() == 3

    with action():
        detach(scope, b)

    assert subtotal.get() == 10.0
    assert selected_quantity.get() == 3
    assert owns(scope, b)
    assert not contains(scope, b)

    with action():
        grant_member(b, lambda item: item.quantity).write(4)

    # The visible aggregate no longer depends on B after detach, while the
    # independent selection still does.
    assert subtotal.get() == 10.0
    assert selected_quantity.get() == 4


def test_same_owned_identity_can_participate_in_multiple_membership_views():
    scope = make_dynamic_line_scope()

    with action():
        line = create_line(
            scope,
            sku="A",
            quantity=2,
            unit_price=5.0,
        )

    favorites = State("favoriteLines", [line])

    with action():
        detach(scope, line)

    assert owns(scope, line)
    assert not contains(scope, line)
    assert favorites.get()[0] == line
    assert read_fact(favorites.get()[0], lambda item: item.sku) == "A"


def test_owner_scoped_candidate_rejects_foreign_membership_without_transfer_semantics():
    first = make_dynamic_line_scope()
    second = make_dynamic_line_scope()

    with action():
        line = create_line(
            first,
            sku="A",
            quantity=2,
            unit_price=5.0,
        )

    with pytest.raises(ValueError, match="not owned"):
        with action():
            attach(second, line)

    assert owns(first, line)
    assert not owns(second, line)


def test_dropping_owner_bookkeeping_does_not_destroy_existing_live_designation():
    scope = make_dynamic_line_scope()

    with action():
        line = create_line(
            scope,
            sku="A",
            quantity=2,
            unit_price=5.0,
        )

    selected = State("selectedLine", line)

    with action():
        drop_owner_record_for_experiment(scope, line)

    assert not owns(scope, line)
    assert not contains(scope, line)

    # Critical falsifier: the current non-owning designation prototype still
    # denotes the live Python-backed target. Therefore "remove from owner list"
    # cannot *by itself* be defined as semantic destruction.
    assert read_fact(selected.get(), lambda item: item.quantity) == 2

    with action():
        grant_member(selected.get(), lambda item: item.quantity).write(8)

    assert read_fact(selected.get(), lambda item: item.quantity) == 8


def test_scope_ownership_and_membership_changes_roll_back_together():
    scope = make_dynamic_line_scope()

    with action():
        existing = create_line(
            scope,
            sku="A",
            quantity=2,
            unit_price=5.0,
        )

    with pytest.raises(ValueError, match="abort"):
        with action():
            detach(scope, existing)
            create_line(
                scope,
                sku="B",
                quantity=3,
                unit_price=7.0,
            )
            raise ValueError("abort")

    assert owns(scope, existing)
    assert contains(scope, existing)
    assert len(scope.owned.get()) == 1
    assert len(scope.order.get()) == 1
