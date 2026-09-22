import pytest

from reference.model import (
    ActionAborted,
    Derived,
    DerivedCycle,
    MutationOutsideAction,
    State,
    action,
)


def test_state_cannot_mutate_outside_action():
    count = State("count", 0)
    with pytest.raises(MutationOutsideAction):
        count.set(1)


def test_action_reads_its_staged_writes_and_commits_atomically():
    a = State("a", 1)
    b = State("b", 2)

    with action():
        a.set(10)
        assert a.get() == 10
        assert b.get() == 2
        b.set(a.get() + 5)

    assert a.get() == 10
    assert b.get() == 15


def test_failed_action_rolls_back():
    count = State("count", 0)

    with pytest.raises(ValueError):
        with action():
            count.set(10)
            raise ValueError("fail")

    assert count.get() == 0


def test_derived_is_cached_and_precisely_invalidated():
    a = State("a", 1)
    b = State("b", 2)
    unrelated = State("unrelated", 9)
    total = Derived("total", lambda: a.get() + b.get())

    assert total.get() == 3
    assert total.evaluations == 1
    assert total.get() == 3
    assert total.evaluations == 1

    with action():
        unrelated.set(10)

    assert total.get() == 3
    assert total.evaluations == 1

    with action():
        a.set(5)

    assert total.get() == 7
    assert total.evaluations == 2


def test_conditional_derived_dependencies_change_dynamically():
    logged_in = State("loggedIn", False)
    name = State("name", "Matt")
    greeting = Derived(
        "greeting",
        lambda: f"Hello, {name.get()}" if logged_in.get() else "Sign in",
    )

    assert greeting.get() == "Sign in"
    assert name not in greeting.dependencies

    with action():
        name.set("Alex")

    # Name was not a dependency while logged out.
    assert greeting.get() == "Sign in"
    assert greeting.evaluations == 1

    with action():
        logged_in.set(True)

    assert greeting.get() == "Hello, Alex"
    assert name in greeting.dependencies
    assert greeting.evaluations == 2

    with action():
        name.set("Sam")

    assert greeting.get() == "Hello, Sam"
    assert greeting.evaluations == 3


def test_conditional_dependency_is_removed_when_branch_changes():
    use_a = State("useA", True)
    a = State("a", 10)
    b = State("b", 20)
    selected = Derived("selected", lambda: a.get() if use_a.get() else b.get())

    assert selected.get() == 10
    assert a in selected.dependencies
    assert b not in selected.dependencies

    with action():
        use_a.set(False)

    assert selected.get() == 20
    assert a not in selected.dependencies
    assert b in selected.dependencies

    before = selected.evaluations
    with action():
        a.set(99)

    assert selected.get() == 20
    assert selected.evaluations == before


def test_derived_chain_invalidation():
    count = State("count", 2)
    doubled = Derived("doubled", lambda: count.get() * 2)
    label = Derived("label", lambda: f"Value: {doubled.get()}")

    assert label.get() == "Value: 4"

    with action():
        count.set(3)

    assert label.get() == "Value: 6"
    assert doubled.evaluations == 2
    assert label.evaluations == 2


def test_diamond_dependency_recomputes_each_node_once():
    a = State("a", 1)
    b = Derived("b", lambda: a.get() + 1)
    c = Derived("c", lambda: a.get() + 2)
    d = Derived("d", lambda: b.get() + c.get())

    assert d.get() == 5
    assert (b.evaluations, c.evaluations, d.evaluations) == (1, 1, 1)

    with action():
        a.set(10)

    assert d.get() == 23
    assert (b.evaluations, c.evaluations, d.evaluations) == (2, 2, 2)


def test_no_op_write_does_not_invalidate():
    count = State("count", 5)
    doubled = Derived("doubled", lambda: count.get() * 2)

    assert doubled.get() == 10
    assert doubled.evaluations == 1

    with action():
        count.set(5)

    assert doubled.get() == 10
    assert doubled.evaluations == 1


def test_nested_actions_join_outer_transaction():
    a = State("a", 0)
    b = State("b", 0)

    with action():
        a.set(1)
        with action():
            assert a.get() == 1
            b.set(2)
        assert b.get() == 2

    assert a.get() == 1
    assert b.get() == 2


def test_failed_nested_action_poison_outer_transaction_even_if_caught():
    a = State("a", 0)
    b = State("b", 0)

    with pytest.raises(ActionAborted):
        with action():
            a.set(1)
            try:
                with action():
                    b.set(2)
                    raise ValueError("inner failure")
            except ValueError:
                pass
            a.set(3)

    assert a.get() == 0
    assert b.get() == 0


def test_failed_derived_evaluation_is_not_cached_and_retries():
    ready = State("ready", False)

    def compute():
        if not ready.get():
            raise ValueError("not ready")
        return 42

    answer = Derived("answer", compute)

    with pytest.raises(ValueError, match="not ready"):
        answer.get()
    with pytest.raises(ValueError, match="not ready"):
        answer.get()

    assert answer.evaluations == 0
    assert answer.attempts == 2

    with action():
        ready.set(True)

    assert answer.get() == 42
    assert answer.evaluations == 1
    assert answer.attempts == 3


def test_state_values_have_value_semantics_not_mutable_aliases():
    original = [1, 2]
    items = State("items", original)

    # Mutating the initializer after declaration cannot mutate state.
    original.append(3)
    assert items.get() == [1, 2]

    # Reading state produces a value, not a hidden writable alias to the slot.
    snapshot = items.get()
    snapshot.append(99)
    assert items.get() == [1, 2]

    # Collection mutation is modeled as read-modify-write in this kernel.
    with action():
        updated = items.get()
        updated.append(4)
        items.set(updated)
        assert items.get() == [1, 2, 4]

    assert items.get() == [1, 2, 4]


def test_set_captures_value_instead_of_aliasing_callers_object():
    items = State("items", [])
    replacement = [1]

    with action():
        items.set(replacement)
        replacement.append(2)
        assert items.get() == [1]

    assert items.get() == [1]


def test_dynamic_cycle_is_detected():
    holder = {}
    a = Derived("a", lambda: holder["b"].get() + 1)
    b = Derived("b", lambda: a.get() + 1)
    holder["b"] = b

    with pytest.raises(DerivedCycle, match="a -> b -> a"):
        a.get()


def test_cycle_can_appear_only_after_conditional_dependency_changes():
    cyclic = State("cyclic", False)
    holder = {}

    a = Derived("a", lambda: holder["b"].get() + 1 if cyclic.get() else 1)
    b = Derived("b", lambda: a.get() + 1)
    holder["b"] = b

    assert a.get() == 1

    with action():
        cyclic.set(True)

    with pytest.raises(DerivedCycle, match="a -> b -> a"):
        a.get()


def test_derived_read_inside_action_sees_staged_state():
    price = State("price", 10)
    quantity = State("quantity", 2)
    total = Derived("total", lambda: price.get() * quantity.get())

    # Establish a committed/global cache first; transaction-local evaluation
    # must not accidentally reuse this stale value.
    assert total.get() == 20

    with action():
        price.set(20)
        assert price.get() == 20
        assert total.get() == 40

    assert price.get() == 20
    assert total.get() == 40


def test_transaction_local_derived_cache_invalidates_after_later_staged_write():
    price = State("price", 10)
    quantity = State("quantity", 2)
    total = Derived("total", lambda: price.get() * quantity.get())

    with action():
        assert total.get() == 20
        before = total.evaluations

        price.set(20)

        assert total.get() == 40
        assert total.evaluations == before + 1


def test_rollback_discards_transaction_local_derived_world():
    price = State("price", 10)
    quantity = State("quantity", 2)
    total = Derived("total", lambda: price.get() * quantity.get())

    assert total.get() == 20

    with pytest.raises(ValueError):
        with action():
            price.set(20)
            assert total.get() == 40
            raise ValueError("abort")

    # The committed world and its cache remain unchanged by the failed action.
    assert price.get() == 10
    assert total.get() == 20


def test_nan_to_nan_is_no_op_for_state_change_detection():
    value = State("value", float("nan"))
    observed = Derived("observed", lambda: value.get())

    assert observed.get() != observed.get()  # ordinary NaN expression behavior
    assert observed.evaluations == 1

    with action():
        value.set(float("nan"))

    observed.get()
    assert observed.evaluations == 1


def test_positive_and_negative_zero_are_equivalent_state_values():
    value = State("value", 0.0)
    observed = Derived("observed", lambda: value.get())

    assert observed.get() == 0.0
    assert observed.evaluations == 1

    with action():
        value.set(-0.0)

    assert observed.get() == 0.0
    assert observed.evaluations == 1


def test_structural_state_equivalence_applies_inside_collections():
    values = State("values", [1.0, float("nan"), {"zero": 0.0}])
    size = Derived("size", lambda: len(values.get()))

    assert size.get() == 3
    assert size.evaluations == 1

    with action():
        values.set([1.0, float("nan"), {"zero": -0.0}])

    assert size.get() == 3
    assert size.evaluations == 1


def test_structurally_different_collection_is_a_state_change():
    values = State("values", [1, 2, 3])
    total = Derived("total", lambda: sum(values.get()))

    assert total.get() == 6

    with action():
        values.set([1, 2, 4])

    assert total.get() == 7
    assert total.evaluations == 2
