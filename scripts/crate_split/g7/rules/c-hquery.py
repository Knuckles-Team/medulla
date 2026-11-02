RANK = {
    "medulla-h-sparql": 1,
    "medulla-h-retrieve": 1,
    "medulla-h-query": 2,
    "medulla-h-mining": 3,
    "medulla-server": 4,
}
RULES = [
    (x, "medulla-h-sparql")
    for x in ["rdf", "sparql_service", "federation", "foreign_source_handlers"]
] + [
    (x, "medulla-h-retrieve")
    for x in [
        "semantic_index",
        "semantic_index_runtime",
        "secondary_indexes",
        "reasoning_projection",
    ]
]
RULES += [("mining", "medulla-h-mining"), ("query::seam", "medulla-server")]
DEFAULT = "medulla-h-query"
