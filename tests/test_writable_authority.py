import pytest

from reference.model import (
    Derived,
    MutationOutsideAction,
    State,
    WritableAuthorityError,
    WritableAuthorityInDerived,
    action,
    writable,
)


def increment(target):
    with action():
        target.write(target.read() + 1)


def swap(left, right):
    with action():
        left_value = left.read()
        right_value = right.read()
        left.write(right_value)
        right.write(left_value)


def test_explicit_authority_can_target_either_of_two_state_slots():
    quantity_a = State("quantityA", 1)
    quantity_b = State("quantityB", 10)
    unrelated = State("unrelated", 99)

    increment(writable(quantity_a))
    assert quantity_a.get() == 2
    assert quantity_b.get() == 10
    assert unrelated.get() == 99

    increment(writable(quantity_b))
    assert quantity_a.get() == 2
    assert quantity_b.get() == 11
    assert unrelated.get() == 99


def test_writable_authority_uses_the_callers_existing_transaction():
    a = State("a", 1)
    b = State("b", 2)
    marker = State("marker", 0)

    with action():
        marker.set(1)
        swap(writable(a), writable(b))

        # The nested operation joined this transaction and its staged writes
        # are immediately visible to the caller.
        assert a.get() == 2
        assert b.get() == 1
        assert marker.get() == 1

    assert a.get() == 2
    assert b.get() == 1
    assert marker.get() == 1


def test_failure_rolls_back_writes_performed_through_authority():
    count = State("count", 5)

    def failing_increment(target):
        with action():
            target.write(target.read() + 1)
            raise ValueError("abort")

    with pytest.raises(ValueError, match="abort"):
        failing_increment(writable(count))

    assert count.get() == 5


def test_writable_authority_does_not_bypass_action_mutation_boundary():
    count = State("count", 0)
    target = writable(count)

    with pytest.raises(MutationOutsideAction):
        target.write(1)

    assert count.get() == 0


def test_writable_authority_is_not_an_ordinary_state_value():
    count = State("count", 0)
    target = writable(count)

    with pytest.raises(WritableAuthorityError, match="not an ordinary Meld value"):
        State("holder", target)

    holder = State("holder", None)
    with pytest.raises(WritableAuthorityError, match="not an ordinary Meld value"):
        with action():
            holder.set(target)

    assert holder.get() is None


def test_writable_authority_cannot_become_a_derived_value():
    count = State("count", 0)
    target = writable(count)
    leaked = Derived("leakedAuthority", lambda: target)

    with pytest.raises(WritableAuthorityError, match="not an ordinary Meld value"):
        leaked.get()

    assert leaked.evaluations == 0


def test_derived_evaluation_cannot_use_writable_authority_even_inside_action():
    count = State("count", 1)
    target = writable(count)

    def mutate_from_derived():
        target.write(target.read() + 1)
        return 0

    bad = Derived("bad", mutate_from_derived)

    with pytest.raises(WritableAuthorityInDerived, match="derived evaluation"):
        with action():
            bad.get()

    assert count.get() == 1


def test_ordinary_state_reads_remain_values_not_writable_authority():
    items = State("items", [1, 2])

    snapshot = items.get()
    snapshot.append(3)

    assert items.get() == [1, 2]
    assert not hasattr(snapshot, "write")
