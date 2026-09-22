import pytest

from reference.experiments.multiple_state_owners import make_invoice_document
from reference.model import State, action, writable


def checkout(document):
    """Validate and check out one document inside the current action world."""

    with action():
        if document.checked_out.get():
            raise ValueError("already checked out")

        if document.quantity.get() <= 0:
            raise ValueError("quantity must be positive")

        if document.quantity.get() > document.limit.get():
            raise ValueError("quantity exceeds limit")

        document.checked_out.set(True)


def set_quantity(target, value):
    """Reusable mutation through explicit writable authority."""

    with action():
        target.write(value)


def test_same_named_document_state_has_distinct_identity():
    a = make_invoice_document(quantity=2, limit=10)
    b = make_invoice_document(quantity=7, limit=20)

    assert a.quantity.name == b.quantity.name == "quantity"
    assert a.limit.name == b.limit.name == "limit"
    assert a.checked_out.name == b.checked_out.name == "checkedOut"
    assert (
        a.remaining_capacity.name
        == b.remaining_capacity.name
        == "remainingCapacity"
    )

    assert a.quantity is not b.quantity
    assert a.limit is not b.limit
    assert a.checked_out is not b.checked_out
    assert a.remaining_capacity is not b.remaining_capacity


def test_derived_dependencies_do_not_leak_between_document_instances():
    a = make_invoice_document(quantity=2, limit=10)
    b = make_invoice_document(quantity=7, limit=20)

    assert a.remaining_capacity.get() == 8
    assert b.remaining_capacity.get() == 13
    assert a.remaining_capacity.evaluations == 1
    assert b.remaining_capacity.evaluations == 1

    with action():
        a.quantity.set(3)

    assert a.remaining_capacity.get() == 7
    assert a.remaining_capacity.evaluations == 2

    # B has the same binding names, but none of its state was read by A's
    # derived computation. Its committed cache must remain valid.
    assert b.remaining_capacity.get() == 13
    assert b.remaining_capacity.evaluations == 1


def test_separate_root_operations_commit_independently():
    a = make_invoice_document(quantity=2, limit=10)
    b = make_invoice_document(quantity=30, limit=20)

    checkout(a)

    assert a.checked_out.get() is True
    assert b.checked_out.get() is False

    with pytest.raises(ValueError, match="quantity exceeds limit"):
        checkout(b)

    # B's later failure must not retroactively affect A's already committed
    # root action.
    assert a.checked_out.get() is True
    assert b.checked_out.get() is False


def test_one_explicit_outer_action_can_span_two_document_instances_atomically():
    a = make_invoice_document(quantity=2, limit=10)
    b = make_invoice_document(quantity=7, limit=20)

    with action():
        checkout(a)
        checkout(b)

        assert a.checked_out.get() is True
        assert b.checked_out.get() is True

    assert a.checked_out.get() is True
    assert b.checked_out.get() is True


def test_cross_document_failure_rolls_back_the_shared_outer_transaction():
    a = make_invoice_document(quantity=2, limit=10)
    b = make_invoice_document(quantity=30, limit=20)

    with pytest.raises(ValueError, match="quantity exceeds limit"):
        with action():
            checkout(a)

            # A's staged write is visible before B fails.
            assert a.checked_out.get() is True

            checkout(b)

    # The intentionally spanning outer action was one transaction, so B's
    # failure rolls back A's staged write as well.
    assert a.checked_out.get() is False
    assert b.checked_out.get() is False


def test_same_named_writable_authority_targets_the_correct_document_state():
    a = make_invoice_document(quantity=2, limit=10)
    b = make_invoice_document(quantity=7, limit=20)

    set_quantity(writable(a.quantity), 4)

    assert a.quantity.get() == 4
    assert b.quantity.get() == 7

    set_quantity(writable(b.quantity), 9)

    assert a.quantity.get() == 4
    assert b.quantity.get() == 9


def test_unrelated_same_named_state_can_coexist_in_one_transaction():
    first = State("status", 1)
    second = State("status", 10)

    with action():
        first.set(2)
        second.set(20)

        assert first.get() == 2
        assert second.get() == 20

    assert first.get() == 2
    assert second.get() == 20


def test_transaction_local_derived_dependencies_remain_instance_isolated():
    a = make_invoice_document(quantity=2, limit=10)
    b = make_invoice_document(quantity=7, limit=20)

    with action():
        # Establish two same-named derived values in the same transaction-local
        # cache/dependency world.
        assert a.remaining_capacity.get() == 8
        assert b.remaining_capacity.get() == 13

        a.quantity.set(4)

        # Only A's temporary derived value should be invalidated.
        assert a.remaining_capacity.get() == 6
        assert b.remaining_capacity.get() == 13

        assert a.remaining_capacity.evaluations == 2
        assert b.remaining_capacity.evaluations == 1

    # Transaction-local derived values are not published into the committed
    # cache. The first post-commit read therefore reevaluates each derived
    # value once. That is cache-lifetime behavior, not cross-instance
    # invalidation.
    assert a.remaining_capacity.get() == 6
    assert b.remaining_capacity.get() == 13
    assert a.remaining_capacity.evaluations == 3
    assert b.remaining_capacity.evaluations == 2
