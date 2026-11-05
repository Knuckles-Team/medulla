# Seam: arbora binds served operations to the engine method catalog

Provider: medulla MEDULLA-CONTRACT-R057
Consumer: arbora ARBORA-IDENTITY-R002.1

Arbora reads the engine's method descriptors and scope registry at run time. It binds each served operation to a catalog row and to a registered scope. No contract file is generated, committed or packaged.

## Wire shape

`medulla.engine.method_catalog()` returns one JSON document. It holds `catalog_digest`, one row per method (`id`, `domain`, `is_wire_callable`, `error_set`, `policy`, `replay_class`, `consumer_profiles`, `stability`, `note`) and one entry per scope (`scope`, `class`, `owner`). The served read `Method::DescribeContract` (`MEDULLA-CONTRACT-R054`) returns the same rows in pages of at most 500 descriptors.

## Refusal codes

A caller without scope `contract:read` receives `ACCESS_DENIED`. A page request above 500 rows receives `INVALID_ARGUMENT`. Arbora treats a method id that has no catalog row as unserved.

## Owner rows

- medulla: MEDULLA-CONTRACT-R057 (the run-time catalog) and MEDULLA-CONTRACT-R054 (the served descriptor read).
- arbora: ARBORA-IDENTITY-R002.1 and ARBORA-OPS-R006 (served operations bound to the catalog).
