"""The synthetic fixtures compare the library lifecycle through the contract enum.

MEDULLA-TYPED-PACKS-R810.3: Python owns no lifecycle transition (the Rust machine
`AgentLibraryMachine` does); the fixtures hold the closed `Lifecycle` enum, compare
`entry.lifecycle` through its members, and refuse a value outside it by naming the field
and the value.
"""

from __future__ import annotations

import inspect

import pytest
from pydantic import ValidationError

from medulla.generated._shared import AgentLibraryLifecycle
from medulla.testing.synthetic import assembly_oracle, decision_export
from medulla.testing.synthetic.assembly import Candidate, Lifecycle
from medulla.testing.synthetic.assembly_generate import generate_gold_set

pytestmark = pytest.mark.no_engine


def _candidate(lifecycle: object) -> Candidate:
    template = generate_gold_set(0).items[0].candidates[0]
    return Candidate.model_validate(template.model_dump() | {"lifecycle": lifecycle})


# spec: MEDULLA-TYPED-PACKS-R810.3
def test_lifecycle_compares_use_enum_members_and_unknown_value_raises() -> None:
    # The members are the contract enum's, generated from the Rust variant list.
    assert {member.value for member in Lifecycle} == {
        member.value for member in AgentLibraryLifecycle
    }
    # The three comparison sites use members, never a string literal.
    for module in (assembly_oracle, decision_export):
        source = inspect.getsource(module)
        assert '.lifecycle == "' not in source, module.__name__
        assert "Lifecycle.PUBLISHED" in source, module.__name__
    # The oracle and the exporter keep exactly the published candidates.
    item = generate_gold_set(0).items[0]
    published = [c for c in item.candidates if c.lifecycle is Lifecycle.PUBLISHED]
    assert tuple(published) == assembly_oracle.universe(item)
    assert {c.component_id for c in decision_export._published_options(item)} == {
        c.component_id for c in published
    }
    # A value outside the enum raises ValueError naming the field and the value.
    with pytest.raises(ValueError) as refused:
        _candidate("archived")
    assert isinstance(refused.value, ValidationError)
    text = str(refused.value)
    assert "lifecycle" in text and "archived" in text
    # A member value is accepted.
    assert _candidate(Lifecycle.RETIRED).lifecycle is Lifecycle.RETIRED
