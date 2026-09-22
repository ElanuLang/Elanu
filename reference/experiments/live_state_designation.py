"""Composition experiment C: non-owning live-state designation.

This module is a non-authoritative semantic laboratory. It deliberately does
not propose Meld source syntax or modify the authoritative reference kernel.

The experiment asks whether Meld can represent "which live state binding is
selected" as an identity-bearing, read-only designation that is:

- distinct from an ordinary detached value;
- distinct from writable authority;
- safe to store in state or return from derived computation;
- resolved to exact writable authority only through an explicit later grant.

The Python object below models identity semantics explicitly. Its custom
``__deepcopy__`` behavior is experimental machinery: ``reference.model`` uses
defensive copying to model ordinary Meld value semantics, while a live
designation must preserve the identity of the existing target rather than
clone that target.
"""

from __future__ import annotations

from typing import Callable, Generic, Protocol, TypeVar

from reference.model import State, WritableState, writable

T = TypeVar("T")
V = TypeVar("V")


class ReadableFact(Protocol[V]):
    """Minimal read surface shared by State and Derived in this experiment."""

    def get(self) -> V:
        ...


class LiveDesignation(Generic[T]):
    """Non-owning identity of one existing live target.

    A designation intentionally has no write operation and is not writable
    authority. Equality here is target identity only so the existing prototype
    state-equivalence machinery can determine whether a stored selection really
    changed. This is not a proposal for Meld's ordinary ``==`` operator.
    """

    __slots__ = ("_target",)

    def __init__(self, target: T) -> None:
        self._target = target

    def __deepcopy__(self, memo: dict[int, object]) -> "LiveDesignation[T]":
        # A designation is immutable and denotes an already-existing live
        # identity. Copying the designation must not clone or detach the target.
        del memo
        return self

    def __eq__(self, other: object) -> bool:
        return isinstance(other, LiveDesignation) and self._target is other._target

    def __hash__(self) -> int:
        return hash(id(self._target))

    def __repr__(self) -> str:
        return f"LiveDesignation(target_id={id(self._target)})"


def designate(target: T) -> LiveDesignation[T]:
    """Create a non-owning designation of an existing live target."""

    return LiveDesignation(target)


def same_target(designation: LiveDesignation[T], target: T) -> bool:
    """Experiment-only identity probe used by tests, not proposed Meld syntax."""

    return designation._target is target


def read_fact(
    designation: LiveDesignation[T],
    select: Callable[[T], ReadableFact[V]],
) -> V:
    """Read one State/Derived fact through a live designation.

    The selected fact performs its normal ``get`` operation, so dependency
    tracking remains attached to the actual live State/Derived identity.
    """

    return select(designation._target).get()


def grant_member(
    designation: LiveDesignation[T],
    select: Callable[[T], State[V]],
) -> WritableState[V]:
    """Explicitly upgrade one selected member to exact writable authority.

    The designation itself remains non-writable. The returned WritableState is
    the existing kernel capability for exactly the State object selected at the
    moment of the grant.
    """

    return writable(select(designation._target))
