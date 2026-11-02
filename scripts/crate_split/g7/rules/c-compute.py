RANK = {
    "medulla-decision-policy": 0,
    "medulla-reasoner": 0,
    "medulla-compute": 1,
    "medulla-finance-market": 1,
    "medulla-finance-quant": 2,
    "medulla-finance": 3,
    "medulla-graph-algos": 2,
    "medulla-mining": 2,
}
RULES = [
    ("finance::market", "medulla-finance-market"),
    ("finance::signals", "medulla-finance-market"),
    ("finance::quant", "medulla-finance-quant"),
    ("finance::derivatives", "medulla-finance-quant"),
    ("finance::statespace", "medulla-finance-quant"),
    ("finance::regime", "medulla-finance-quant"),
    ("finance::signal_models", "medulla-finance-quant"),
    ("finance::factor", "medulla-finance-quant"),
    ("finance::optimizer", "medulla-finance-quant"),
    ("finance::risk", "medulla-finance-quant"),
    ("finance::leverage", "medulla-finance-quant"),
    ("finance*", "medulla-finance"),
    ("graph_algos", "medulla-graph-algos"),
    ("algorithms", "medulla-graph-algos"),
    ("graphlearn", "medulla-graph-algos"),
    ("contract", "medulla-graph-algos"),
    ("mining", "medulla-mining"),
    ("solve", "medulla-decision-policy"),
    ("reasoning*", "medulla-reasoner"),
]
DEFAULT = "medulla-compute"
CUTS = {
    f"finance::quant::{x}": "medulla-finance-market"
    for x in [
        "CvSplit",
        "deflated_sharpe_ratio",
        "probability_of_backtest_overfit",
        "purged_cpcv_splits",
        "*",
    ]
}
