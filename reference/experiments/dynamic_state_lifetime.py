"""Composition experiment E: dynamic modeled-state membership and lifetime.

This is a non-authoritative semantic laboratory. It does not propose Meld
collection syntax, allocation syntax, memory management, or a permanent
ownership construct.

The experiment separates three concepts that the reorder experiment showed must
not be collapsed accidentally:

- physical host allocation of an experimental object;
- semantic ownership/lifetime of a live state identity;
- ordered membership/visibility of that identity in an application structure.

A ``DynamicLineScope`` is experiment-only scaffolding for a possible
state-world lifetime scope. ``owned`` records which dynamically created line
identities belong to the scope. ``order`` records the currently visible ordered
membership. Both are ordinary Meld ``State`` values in the Python kernel, so
changes to ownership registration and membership participate in the existing
action transaction and rollback rules.

The underlying Python object may physically exist even when an action rolls
back. That host-level fact is deliberately not treated as Meld semantic
liveness. The experiment asks whether committed ownership registration can be
the semantic boundary without choosing a garbage collector, ARC, borrowing, or
another memory-management strategy.
"""

from __future__ import annotations

from dataclasses import dataclass

from reference.experiments.identity_under_reorder import LineItem, make_line_item
from reference.experiments.live_state_designation import LiveDesignation, designate
from reference.model import State


@dataclass(frozen=True)
class DynamicLineScope:
    """Experiment-only lifetime scope plus one ordered membership view."""

    owned: State[list[LiveDesignation[LineItem]]]
    order: State[list[LiveDesignation[LineItem]]]


def make_dynamic_line_scope() -> DynamicLineScope:
    return DynamicLineScope(
        owned=State("ownedLines", []),
        order=State("lines", []),
    )


def owns(scope: DynamicLineScope, line: LiveDesignation[LineItem]) -> bool:
    return line in scope.owned.get()


def contains(scope: DynamicLineScope, line: LiveDesignation[LineItem]) -> bool:
    return line in scope.order.get()


def create_line(
    scope: DynamicLineScope,
    *,
    sku: str,
    quantity: int,
    unit_price: float,
    visible: bool = True,
) -> LiveDesignation[LineItem]:
    """Create and stage registration of one new line identity in ``scope``.

    ``State.set`` enforces the existing action boundary. The Python allocation
    happens before those writes, but semantic ownership is represented by the
    staged ``owned`` value. A failed action therefore leaves no committed scope
    registration even if Python still has an implementation-level object.
    """

    item = make_line_item(sku=sku, quantity=quantity, unit_price=unit_price)
    line = designate(item)

    scope.owned.set([*scope.owned.get(), line])
    if visible:
        scope.order.set([*scope.order.get(), line])

    return line


def detach(scope: DynamicLineScope, line: LiveDesignation[LineItem]) -> None:
    """Remove ordered membership without changing scope ownership."""

    if not owns(scope, line):
        raise ValueError("line is not owned by this scope")

    scope.order.set([candidate for candidate in scope.order.get() if candidate != line])


def attach(
    scope: DynamicLineScope,
    line: LiveDesignation[LineItem],
    *,
    index: int | None = None,
) -> None:
    """Add an already-owned identity to ordered membership.

    Rejecting foreign identities is a candidate scope discipline used only by
    this experiment. It is not yet Meld language law.
    """

    if not owns(scope, line):
        raise ValueError("line is not owned by this scope")

    current = [candidate for candidate in scope.order.get() if candidate != line]
    if index is None:
        current.append(line)
    else:
        current.insert(index, line)
    scope.order.set(current)


def drop_owner_record_for_experiment(
    scope: DynamicLineScope,
    line: LiveDesignation[LineItem],
) -> None:
    """Remove scope bookkeeping to pressure-test destruction semantics.

    This function intentionally does *not* claim to destroy the target. The
    current ``LiveDesignation`` prototype still denotes the Python object after
    this bookkeeping change. Tests use that fact as a falsifier: if future Meld
    wants ownership removal to end semantic lifetime, it needs explicit
    invalidation/lifetime semantics beyond ordinary structural membership.
    """

    scope.order.set([candidate for candidate in scope.order.get() if candidate != line])
    scope.owned.set([candidate for candidate in scope.owned.get() if candidate != line])
