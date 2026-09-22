"""Executable semantic model for Elanu's current semantic kernel.

This is intentionally small and explicit. It is a behavioral prototype, not
an implementation plan for the eventual compiler/runtime.
"""

from __future__ import annotations

from contextlib import contextmanager
from contextvars import ContextVar
from copy import deepcopy
from dataclasses import dataclass, fields, field, is_dataclass
from math import isnan
from typing import Any, Callable, Generic, TypeVar

T = TypeVar("T")

_current_tracker: ContextVar[Any | None] = ContextVar(
    "meld_current_tracker", default=None
)
_current_tx: ContextVar["Transaction | None"] = ContextVar("meld_current_tx", default=None)
_eval_stack: ContextVar[tuple["Derived[Any]", ...]] = ContextVar(
    "meld_eval_stack", default=()
)


class MeldSemanticError(RuntimeError):
    pass


class MutationOutsideAction(MeldSemanticError):
    pass


class DerivedCycle(MeldSemanticError):
    pass


class ActionAborted(MeldSemanticError):
    pass


class WritableAuthorityError(MeldSemanticError):
    """Writable state authority was used as an ordinary Meld value."""


class WritableAuthorityInDerived(WritableAuthorityError):
    """Writable state authority was exercised during derived evaluation."""


class Dependency:
    dependents: set["Derived[Any]"]




def _state_equivalent(left: Any, right: Any) -> bool:
    """Semantic equivalence used for Meld state-change detection.

    This is intentionally distinct from the language's eventual `==` operator.
    In particular, NaN is equivalent to NaN for state invalidation, and nested
    value containers compare structurally using the same rule.

    The semantic kernel models value payloads only. Future resource/reference
    types will require identity semantics in the type system rather than being
    inferred here from arbitrary Python objects.
    """

    if isinstance(left, float) and isinstance(right, float):
        if isnan(left) and isnan(right):
            return True
        # Python already treats +0.0 and -0.0 as equal, matching Meld's rule.
        return left == right

    if type(left) is not type(right):
        return False

    if isinstance(left, (list, tuple)):
        return len(left) == len(right) and all(
            _state_equivalent(a, b) for a, b in zip(left, right)
        )

    if isinstance(left, dict):
        if left.keys() != right.keys():
            return False
        return all(_state_equivalent(left[key], right[key]) for key in left)

    if isinstance(left, (set, frozenset)):
        if len(left) != len(right):
            return False
        unmatched = list(right)
        for item in left:
            for index, candidate in enumerate(unmatched):
                if _state_equivalent(item, candidate):
                    unmatched.pop(index)
                    break
            else:
                return False
        return True

    if is_dataclass(left) and not isinstance(left, type):
        return all(
            _state_equivalent(getattr(left, f.name), getattr(right, f.name))
            for f in fields(left)
        )

    return left == right


def _value_copy(value: T) -> T:
    """Model Meld value semantics using defensive copies in Python.

    This is a semantic convenience for the prototype, not a proposed runtime
    strategy. A real compiler can use immutable values, ownership, copy-on-write,
    structural sharing, or other representations while preserving the same
    observable behavior.
    """

    return deepcopy(value)


@dataclass
class _TransactionTracker:
    tx: "Transaction"
    derived: "Derived[Any]"

    def _register_dependency(self, dep: Dependency) -> None:
        self.tx.register_dependency(self.derived, dep)


@dataclass(eq=False)
class State(Generic[T], Dependency):
    name: str
    _value: T
    dependents: set["Derived[Any]"] = field(default_factory=set)

    def __post_init__(self) -> None:
        self._value = _value_copy(self._value)

    def get(self) -> T:
        tracker = _current_tracker.get()
        if tracker is not None:
            tracker._register_dependency(self)

        tx = _current_tx.get()
        if tx is not None and self in tx.writes:
            return _value_copy(tx.writes[self])
        return _value_copy(self._value)

    def set(self, value: T) -> None:
        tx = _current_tx.get()
        if tx is None:
            raise MutationOutsideAction(
                f"state '{self.name}' can only be mutated inside an action"
            )
        tx.writes[self] = _value_copy(value)
        tx.invalidate_dependency(self)


@dataclass(frozen=True)
class WritableState(Generic[T]):
    """Explicit authority to read and write one specific State slot.

    Writable authority is not an ordinary Meld value. It exists to model
    explicit caller-granted mutation authority for action parameters.
    """

    _state: State[T]

    @property
    def name(self) -> str:
        return self._state.name

    def _reject_derived_use(self) -> None:
        if _current_tracker.get() is not None:
            raise WritableAuthorityInDerived(
                "writable state authority is not available during derived evaluation"
            )

    def read(self) -> T:
        self._reject_derived_use()
        return self._state.get()

    def write(self, value: T) -> None:
        self._reject_derived_use()
        self._state.set(value)

    def __deepcopy__(self, memo: dict[int, object]) -> "WritableState[T]":
        del memo
        raise WritableAuthorityError(
            "writable state authority is not an ordinary Meld value"
        )


def writable(state: State[T]) -> WritableState[T]:
    """Explicitly grant writable authority for one State slot."""

    return WritableState(state)


@dataclass(eq=False)
class Derived(Generic[T], Dependency):
    name: str
    compute: Callable[[], T]
    dependents: set["Derived[Any]"] = field(default_factory=set)
    dependencies: set[Dependency] = field(default_factory=set)
    _valid: bool = False
    _value: T | None = None
    evaluations: int = 0
    attempts: int = 0

    def _register_dependency(self, dep: Dependency) -> None:
        self.dependencies.add(dep)
        dep.dependents.add(self)

    def _clear_dependencies(self) -> None:
        for dep in self.dependencies:
            dep.dependents.discard(self)
        self.dependencies.clear()

    def invalidate(self) -> None:
        if not self._valid:
            return
        self._valid = False
        for dependent in tuple(self.dependents):
            dependent.invalidate()

    def get(self) -> T:
        tracker = _current_tracker.get()
        if tracker is not None:
            owner = getattr(tracker, "derived", tracker)
            if owner is not self:
                tracker._register_dependency(self)

        tx = _current_tx.get()
        if tx is not None:
            if self in tx.derived_values:
                return _value_copy(tx.derived_values[self])

            stack = _eval_stack.get()
            if self in stack:
                cycle = " -> ".join([d.name for d in stack] + [self.name])
                raise DerivedCycle(f"cyclic derived state: {cycle}")

            tx.clear_dependencies(self)
            token_stack = _eval_stack.set(stack + (self,))
            token_tracker = _current_tracker.set(_TransactionTracker(tx, self))
            self.attempts += 1
            try:
                value = self.compute()
            finally:
                _current_tracker.reset(token_tracker)
                _eval_stack.reset(token_stack)

            tx.derived_values[self] = _value_copy(value)
            self.evaluations += 1
            return _value_copy(value)

        if self._valid:
            return _value_copy(self._value)  # type: ignore[arg-type]

        stack = _eval_stack.get()
        if self in stack:
            cycle = " -> ".join([d.name for d in stack] + [self.name])
            raise DerivedCycle(f"cyclic derived state: {cycle}")

        self._clear_dependencies()
        token_stack = _eval_stack.set(stack + (self,))
        token_tracker = _current_tracker.set(self)
        self.attempts += 1
        try:
            value = self.compute()
        finally:
            _current_tracker.reset(token_tracker)
            _eval_stack.reset(token_stack)

        self._value = _value_copy(value)
        self._valid = True
        self.evaluations += 1
        return _value_copy(value)


@dataclass
class Transaction:
    writes: dict[State[Any], Any] = field(default_factory=dict)
    aborted: bool = False
    derived_values: dict[Derived[Any], Any] = field(default_factory=dict)
    derived_dependencies: dict[Derived[Any], set[Dependency]] = field(default_factory=dict)
    dependents: dict[Dependency, set[Derived[Any]]] = field(default_factory=dict)

    def register_dependency(self, derived: Derived[Any], dep: Dependency) -> None:
        self.derived_dependencies.setdefault(derived, set()).add(dep)
        self.dependents.setdefault(dep, set()).add(derived)

    def clear_dependencies(self, derived: Derived[Any]) -> None:
        for dep in self.derived_dependencies.pop(derived, set()):
            watchers = self.dependents.get(dep)
            if watchers is not None:
                watchers.discard(derived)
                if not watchers:
                    self.dependents.pop(dep, None)

    def invalidate_dependency(self, dep: Dependency) -> None:
        for derived in tuple(self.dependents.get(dep, set())):
            self.derived_values.pop(derived, None)
            self.invalidate_dependency(derived)

    def commit(self) -> set[State[Any]]:
        changed: set[State[Any]] = set()
        for state, new_value in self.writes.items():
            if not _state_equivalent(state._value, new_value):
                state._value = _value_copy(new_value)
                changed.add(state)

        for state in changed:
            for dependent in tuple(state.dependents):
                dependent.invalidate()

        return changed


@contextmanager
def action():
    """Create/join an atomic mutation boundary.

    Nested actions join the outer transaction. If an exception escapes any
    nested action, the shared transaction becomes rollback-only even if an outer
    action catches that exception.
    """

    existing = _current_tx.get()
    if existing is not None:
        try:
            yield existing
        except Exception:
            existing.aborted = True
            raise
        return

    tx = Transaction()
    token = _current_tx.set(tx)
    try:
        yield tx
    except Exception:
        tx.aborted = True
        raise
    else:
        if tx.aborted:
            raise ActionAborted(
                "action transaction was aborted by a failed nested action"
            )
        tx.commit()
    finally:
        _current_tx.reset(token)
