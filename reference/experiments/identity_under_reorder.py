"""Composition experiment D: stable identity under ordered structural change.

This is a non-authoritative semantic laboratory. It deliberately does not
propose Meld collection syntax, collection ownership, dynamic allocation, or a
new identity primitive.

The experiment asks whether the existing semantic kernel plus the experimental
non-owning live-state designation can preserve logical line-item identity while
an ordered structural value changes position/membership.

Python lists are only host scaffolding for ordered membership. A list stored in
``State`` is modeled as an ordinary structural value. Its elements are
``LiveDesignation`` values so copying/reordering membership does not clone the
live line-item state worlds they designate.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Iterable

from reference.experiments.live_state_designation import LiveDesignation, designate
from reference.model import Derived, State


@dataclass(frozen=True)
class LineItem:
    """One conceptual line-item state world used only by this experiment."""

    sku: State[str]
    quantity: State[int]
    unit_price: State[float]
    line_total: Derived[float]


def make_line_item(*, sku: str, quantity: int, unit_price: float) -> LineItem:
    """Create one line item with same-named members across every realization."""

    sku_state = State("sku", sku)
    quantity_state = State("quantity", quantity)
    unit_price_state = State("unitPrice", unit_price)
    line_total = Derived(
        "lineTotal",
        lambda: quantity_state.get() * unit_price_state.get(),
    )

    return LineItem(
        sku=sku_state,
        quantity=quantity_state,
        unit_price=unit_price_state,
        line_total=line_total,
    )


def ordered_membership(items: Iterable[LineItem]) -> list[LiveDesignation[LineItem]]:
    """Create an ordered structural value designating the supplied live items."""

    return [designate(item) for item in items]
