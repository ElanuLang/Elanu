import pytest

from reference.experiments.live_state_designation import (
    designate,
    grant_member,
    read_fact,
    same_target,
)
from reference.experiments.multiple_state_owners import make_invoice_document
from reference.model import Derived, State, action, writable


def test_designation_preserves_live_target_identity_through_value_copying():
    invoice = make_invoice_document(quantity=2, limit=10)
    designation = designate(invoice)

    selected = State("selectedInvoice", designation)

    # State's defensive-copy machinery must copy the designation without
    # cloning/detaching the live target it denotes.
    read_back = selected.get()
    assert read_back is designation
    assert same_target(read_back, invoice)


def test_designation_itself_carries_no_writable_operation():
    invoice = make_invoice_document(quantity=2, limit=10)
    designation = designate(invoice)

    assert not hasattr(designation, "write")
    assert not hasattr(designation, "set")

    # It can still read a live fact through the experiment's read-only path.
    assert read_fact(designation, lambda document: document.quantity) == 2


def test_derived_designation_switches_live_dependencies_to_the_selected_target():
    a = make_invoice_document(quantity=2, limit=10)
    b = make_invoice_document(quantity=7, limit=20)
    choose_a = State("chooseA", True)

    selected = Derived(
        "selectedInvoice",
        lambda: designate(a) if choose_a.get() else designate(b),
    )
    selected_quantity = Derived(
        "selectedQuantity",
        lambda: read_fact(selected.get(), lambda document: document.quantity),
    )

    assert selected_quantity.get() == 2
    assert selected_quantity.evaluations == 1

    with action():
        a.quantity.set(3)

    assert selected_quantity.get() == 3
    assert selected_quantity.evaluations == 2

    with action():
        choose_a.set(False)

    assert selected_quantity.get() == 7
    assert selected_quantity.evaluations == 3

    # Re-evaluation after the selection change must replace the old target
    # dependency. A no longer participates in selectedQuantity.
    with action():
        a.quantity.set(4)

    assert selected_quantity.get() == 7
    assert selected_quantity.evaluations == 3

    with action():
        b.quantity.set(8)

    assert selected_quantity.get() == 8
    assert selected_quantity.evaluations == 4


def test_stored_selection_is_separate_mutable_state_from_the_selected_target():
    a = make_invoice_document(quantity=2, limit=10)
    b = make_invoice_document(quantity=7, limit=20)
    selected = State("selectedInvoice", designate(a))

    selected_quantity = Derived(
        "selectedQuantity",
        lambda: read_fact(selected.get(), lambda document: document.quantity),
    )

    assert selected_quantity.get() == 2

    # Writable authority to the selection slot changes which live target is
    # selected. It does not mutate either invoice merely by rebinding selection.
    with action():
        writable(selected).write(designate(b))

    assert a.quantity.get() == 2
    assert b.quantity.get() == 7
    assert same_target(selected.get(), b)
    assert selected_quantity.get() == 7

    # Explicit authority resolved through the designation targets B's member.
    with action():
        grant_member(selected.get(), lambda document: document.quantity).write(9)

    assert a.quantity.get() == 2
    assert b.quantity.get() == 9
    assert selected_quantity.get() == 9


def test_reselecting_same_live_target_is_semantically_a_no_op():
    a = make_invoice_document(quantity=2, limit=10)
    b = make_invoice_document(quantity=7, limit=20)
    selected = State("selectedInvoice", designate(a))

    selected_quantity = Derived(
        "selectedQuantity",
        lambda: read_fact(selected.get(), lambda document: document.quantity),
    )

    assert selected_quantity.get() == 2
    assert selected_quantity.evaluations == 1

    # A fresh designation object for the same target denotes the same live
    # identity and should therefore not invalidate committed dependents.
    with action():
        selected.set(designate(a))

    assert selected_quantity.get() == 2
    assert selected_quantity.evaluations == 1

    with action():
        selected.set(designate(b))

    assert selected_quantity.get() == 7
    assert selected_quantity.evaluations == 2


def test_explicit_authority_grant_resolves_to_exact_identity_at_grant_time():
    a = make_invoice_document(quantity=2, limit=10)
    b = make_invoice_document(quantity=7, limit=20)
    selected = State("selectedInvoice", designate(a))

    with action():
        authority = grant_member(
            selected.get(),
            lambda document: document.quantity,
        )

        # Change the stored selection after authority has already been granted.
        writable(selected).write(designate(b))

        # The existing authority remains exact authority to A.quantity rather
        # than becoming a moving pointer that follows the selection slot.
        authority.write(5)

    assert same_target(selected.get(), b)
    assert a.quantity.get() == 5
    assert b.quantity.get() == 7


def test_transaction_local_derived_designation_uses_staged_selection():
    a = make_invoice_document(quantity=2, limit=10)
    b = make_invoice_document(quantity=7, limit=20)
    choose_a = State("chooseA", True)

    selected = Derived(
        "selectedInvoice",
        lambda: designate(a) if choose_a.get() else designate(b),
    )

    with action():
        choose_a.set(False)
        authority = grant_member(
            selected.get(),
            lambda document: document.quantity,
        )
        authority.write(11)

    assert choose_a.get() is False
    assert a.quantity.get() == 2
    assert b.quantity.get() == 11


def test_failure_rolls_back_selection_and_designated_target_writes_together():
    a = make_invoice_document(quantity=2, limit=10)
    b = make_invoice_document(quantity=7, limit=20)
    selected = State("selectedInvoice", designate(a))

    with pytest.raises(ValueError, match="abort"):
        with action():
            grant_member(
                selected.get(),
                lambda document: document.quantity,
            ).write(99)
            writable(selected).write(designate(b))
            raise ValueError("abort")

    assert same_target(selected.get(), a)
    assert a.quantity.get() == 2
    assert b.quantity.get() == 7
