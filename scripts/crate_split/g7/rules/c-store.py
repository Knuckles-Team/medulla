RANK = {
    "medulla-store": 1,
    "medulla-work-store": 2,
    "medulla-resource-store": 2,
    "medulla-store-apply": 3,
    "medulla-lane-store": 4,
}
RULES = (
    [
        (x, "medulla-work-store")
        for x in [
            "redb_store::work_item",
            "redb_store::work_item_capability",
            "redb_store::store_rows",
        ]
    ]
    + [
        (x, "medulla-resource-store")
        for x in [
            "redb_store::resource",
            "redb_store::resource_reservation_tests",
            "redb_store::capacity_lease",
        ]
    ]
    + [("redb_store::development_lane", "medulla-lane-store")]
    + [
        (x, "medulla-store-apply")
        for x in [
            "redb_store::store_native",
            "redb_store::store_control",
            "redb_store::store_cleanup",
            "redb_store::checkpoint",
        ]
    ]
)
RULES += [
    ("redb_store::security_tests", "medulla-lane-store"),
    ("redb_store::mutation_batch_tests", "medulla-store-apply"),
]
DEFAULT = "medulla-store"
