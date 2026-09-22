"""Composition experiment B: multiple independently instantiated state owners.

This is a non-authoritative semantic laboratory, not proposed Meld syntax or a
new owner/domain primitive.

The application-level InvoiceDocument object deliberately groups independently
created State and Derived objects whose internal Meld names are identical across
instances. The experiment asks whether the existing semantic kernel already
keeps their identity, dependencies, and transactions coherent before Meld adds
any source-level representation for runtime-created owners.
"""

from __future__ import annotations

from dataclasses import dataclass

from reference.model import Derived, State


@dataclass(frozen=True)
class InvoiceDocument:
    """Application scaffolding for one conceptual document instance.

    This Python object is not a proposed Meld ownership abstraction. It merely
    gives the experiment a convenient way to keep the state belonging to one
    conceptual document together.
    """

    quantity: State[int]
    limit: State[int]
    checked_out: State[bool]
    remaining_capacity: Derived[int]


def make_invoice_document(*, quantity: int, limit: int) -> InvoiceDocument:
    """Create one independent invoice-like state world.

    Every instance intentionally uses the same internal binding names. Distinct
    behavior must therefore come from semantic state identity rather than from
    globally unique names such as invoiceAQuantity / invoiceBQuantity.
    """

    quantity_state = State("quantity", quantity)
    limit_state = State("limit", limit)
    checked_out_state = State("checkedOut", False)

    remaining_capacity = Derived(
        "remainingCapacity",
        lambda: limit_state.get() - quantity_state.get(),
    )

    return InvoiceDocument(
        quantity=quantity_state,
        limit=limit_state,
        checked_out=checked_out_state,
        remaining_capacity=remaining_capacity,
    )
