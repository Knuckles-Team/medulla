"""MEDULLA-DECISION-ENGINE-R805.14: the Python mirror of the typed promotion answer."""

from __future__ import annotations

import pytest
from pydantic import ValidationError

from medulla.generated.models import (
    FamilyAnswerPromotion,
    PromotionAnswer,
    PromotionAnswerVerdict,
)

pytestmark = pytest.mark.no_engine


@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R805.14")
def test_the_promotion_answer_mirrors_the_wire_shape() -> None:
    sealed = FamilyAnswerPromotion.model_validate(
        {
            "type": "promotion",
            "value": {
                "verdict": "promote",
                "champion_ref": "revision:one",
                "challenger_ref": "revision:two",
            },
        }
    )
    assert sealed.value == PromotionAnswer(
        verdict=PromotionAnswerVerdict.PROMOTE,
        champion_ref="revision:one",
        challenger_ref="revision:two",
    )
    refused = PromotionAnswer.model_validate(
        {
            "verdict": "abstain",
            "challenger_ref": "revision:three",
            "reason": "PROMOTION_UNEVALUATED",
        }
    )
    assert refused.champion_ref is None
    assert refused.reason == "PROMOTION_UNEVALUATED"
    assert {verdict.value for verdict in PromotionAnswerVerdict} == {
        "promote",
        "hold",
        "rollback",
        "ramp",
        "abstain",
    }
    with pytest.raises(ValidationError):
        PromotionAnswer.model_validate({"verdict": "approve", "challenger_ref": "x"})
    with pytest.raises(ValidationError):
        PromotionAnswer.model_validate(
            {"verdict": "hold", "challenger_ref": "x", "unknown": 1}
        )
