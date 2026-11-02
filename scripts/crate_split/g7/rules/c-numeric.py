RANK = {
    "medulla-numeric": 1,
    "medulla-decision-stats": 2,
    "medulla-finance-quant": 3,
    "medulla-finance": 4,
}
RULES = [(x, "medulla-decision-stats") for x in ["decision", "conformal", "ope"]] + [
    ("execution", "medulla-finance-quant"),
    ("sizing", "medulla-finance-quant"),
    ("attribution", "medulla-finance"),
    ("banking", "medulla-finance"),
]
DEFAULT = "medulla-numeric"
