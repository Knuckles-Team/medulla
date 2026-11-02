RANK = {"medulla-raft": 1, "medulla-raft-xshard": 2}
RULES = [
    (x, "medulla-raft-xshard")
    for x in [
        "raft::cross_shard_txn",
        "raft::cross_node_elasticity",
        "raft::exchange",
        "raft::pregel",
        "raft::drain",
    ]
] + [
    (x, "medulla-raft-xshard")
    for x in [
        "raft::harness",
        "raft::harness_support",
        "raft::xshard_harness",
        "raft::xshard_modality_harness",
        "raft::xread_harness",
        "raft::reshard_harness",
        "raft::tests",
        "raft::flake_budget",
    ]
]
DEFAULT = "medulla-raft"
