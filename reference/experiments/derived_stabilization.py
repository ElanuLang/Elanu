"""Experiment-only derived stabilization model.

This module pressure-tests one possible refinement of Meld's reactive cache law
without changing the authoritative reference kernel in ``reference.model``.

The candidate keeps derived evaluation lazy. A committed state change marks its
direct derived consumers definitely dirty and marks downstream derived values
suspect while retaining their cached values.

A suspect downstream value attempts stabilization only when it has exactly one
suspect derived dependency and no definite direct change of its own. If that
single dependency stabilizes to an equivalent observable value, the downstream
cache remains valid without reevaluating its computation. If the dependency
changes, or if multiple causes are suspect, the downstream computation
reevaluates normally and dynamic control flow chooses the current dependency
set.

This conservative rule deliberately gives up some possible cache preservation in
exchange for avoiding speculative evaluation of old dependencies. It therefore
does not require historical dependency-read order as part of the candidate.

Transaction-local evaluation deliberately keeps the existing eager temporary
invalidation behavior. This experiment asks whether committed-cache
stabilization composes with the current transaction law; it does not claim that
transaction-local caches must receive the same optimization.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any, Callable, Generic, TypeVar

from reference.model import (
    Dependency,
    Derived,
    DerivedCycle,
    _current_tracker,
    _current_tx,
    _eval_stack,
    _state_equivalent,
    _value_copy,
)

T = TypeVar("T")


@dataclass
class _LabTransactionTracker:
    tx: Any
    derived: "StabilizingDerived[Any]"

    def _register_dependency(self, dep: Dependency) -> None:
        self.tx.register_dependency(self.derived, dep)


class StabilizingDerived(Derived[T], Generic[T]):
    """Experimental lazy derived value with observable-value stabilization.

    ``invalidate()`` is still the entry point used by the authoritative
    transaction commit code for a directly changed state dependency. The
    experiment interprets that call as a *definite* reason to recompute this
    node, but only propagates *suspect* status to downstream derived values.
    """

    def __init__(self, name: str, compute: Callable[[], T]) -> None:
        super().__init__(name=name, compute=compute)
        self._dirty = False
        self._definite_dirty = False
        self._suspect_causes: set[StabilizingDerived[Any]] = set()

    @property
    def dirty(self) -> bool:
        """Experiment-only probe used by pressure tests."""

        return self._dirty

    def invalidate(self) -> None:
        """Mark a direct committed dependency change.

        ``Transaction.commit`` calls this method only on direct dependents of a
        state whose committed value really changed. Downstream propagation is
        handled separately as suspicion so an unchanged derived value can later
        stabilize the graph.
        """

        if not self._valid:
            return

        was_dirty = self._dirty
        self._dirty = True
        self._definite_dirty = True
        if not was_dirty:
            self._propagate_suspect()

    def _mark_suspect(self, cause: "StabilizingDerived[Any]") -> None:
        if not self._valid:
            return

        was_dirty = self._dirty
        self._dirty = True
        self._suspect_causes.add(cause)
        if not was_dirty:
            self._propagate_suspect()

    def _mark_changed(self, cause: "StabilizingDerived[Any]") -> None:
        if not self._valid:
            return

        was_dirty = self._dirty
        self._dirty = True
        self._suspect_causes.discard(cause)
        self._definite_dirty = True
        if not was_dirty:
            self._propagate_suspect()

    def _mark_stable(self, cause: "StabilizingDerived[Any]") -> None:
        if not self._valid:
            return

        self._suspect_causes.discard(cause)
        if self._dirty and not self._definite_dirty and not self._suspect_causes:
            self._dirty = False
            self._propagate_stable()

    def _propagate_suspect(self) -> None:
        for dependent in tuple(self.dependents):
            if isinstance(dependent, StabilizingDerived):
                dependent._mark_suspect(self)
            else:
                # Mixing the experiment with an authoritative Derived falls
                # back to the existing eager invalidation rule.
                dependent.invalidate()

    def _propagate_stable(self) -> None:
        for dependent in tuple(self.dependents):
            if isinstance(dependent, StabilizingDerived):
                dependent._mark_stable(self)

    def _propagate_changed(self) -> None:
        for dependent in tuple(self.dependents):
            if isinstance(dependent, StabilizingDerived):
                dependent._mark_changed(self)
            else:
                dependent.invalidate()

    def get(self) -> T:
        tracker = _current_tracker.get()
        if tracker is not None:
            owner = getattr(tracker, "derived", tracker)
            if owner is not self:
                tracker._register_dependency(self)

        tx = _current_tx.get()
        if tx is not None:
            return self._get_transactional(tx)

        if self._valid and not self._dirty:
            return _value_copy(self._value)  # type: ignore[arg-type]

        return self._ensure_current()

    def _get_transactional(self, tx: Any) -> T:
        """Preserve the authoritative transaction-local cache behavior."""

        if self in tx.derived_values:
            return _value_copy(tx.derived_values[self])

        stack = _eval_stack.get()
        if self in stack:
            cycle = " -> ".join([d.name for d in stack] + [self.name])
            raise DerivedCycle(f"cyclic derived state: {cycle}")

        tx.clear_dependencies(self)
        token_stack = _eval_stack.set(stack + (self,))
        token_tracker = _current_tracker.set(_LabTransactionTracker(tx, self))
        self.attempts += 1
        try:
            value = self.compute()
        finally:
            _current_tracker.reset(token_tracker)
            _eval_stack.reset(token_stack)

        tx.derived_values[self] = _value_copy(value)
        self.evaluations += 1
        return _value_copy(value)

    def _ensure_current(self) -> T:
        if self._valid and not self._dirty:
            return _value_copy(self._value)  # type: ignore[arg-type]

        stack = _eval_stack.get()
        if self in stack:
            cycle = " -> ".join([d.name for d in stack] + [self.name])
            raise DerivedCycle(f"cyclic derived state: {cycle}")

        token_stack = _eval_stack.set(stack + (self,))
        try:
            if not self._valid:
                return self._compute_committed(had_old_value=False)

            # Only one suspect derived cause is revalidated speculatively. If
            # there are multiple causes, recompute this node normally instead
            # of traversing an old dependency graph whose control flow may no
            # longer be relevant.
            if not self._definite_dirty and len(self._suspect_causes) == 1:
                cause = next(iter(self._suspect_causes))
                cause._ensure_current()
                if not self._dirty:
                    return _value_copy(self._value)  # type: ignore[arg-type]

            if self._dirty and not self._definite_dirty:
                self._definite_dirty = True

            return self._compute_committed(had_old_value=True)
        finally:
            _eval_stack.reset(token_stack)

    def _compute_committed(self, *, had_old_value: bool) -> T:
        old_value = _value_copy(self._value) if had_old_value else None

        self._clear_dependencies()
        token_tracker = _current_tracker.set(self)
        self.attempts += 1
        try:
            value = self.compute()
        finally:
            _current_tracker.reset(token_tracker)

        value_copy = _value_copy(value)
        changed = had_old_value and not _state_equivalent(old_value, value_copy)

        self._value = value_copy
        self._valid = True
        self._dirty = False
        self._definite_dirty = False
        self._suspect_causes.clear()
        self.evaluations += 1

        if had_old_value:
            if changed:
                self._propagate_changed()
            else:
                self._propagate_stable()

        return _value_copy(value_copy)
