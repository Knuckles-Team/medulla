# Seam: Gramps native-hosting pilot corpus and phase exits

Provider: medulla MEDULLA-UNIFIED-DATA-PLANE-R026.2.2
Consumer: gramps-mcp CONN-GRAMPS-MCP-CONNECTOR-CONTENT-R001

## Wire shape

The connector side supplies a synthetic import and export tree, the real-Postgres control corpus and the application driver behavior. Medulla records an export digest, replays the statement corpus against an unmodified tenant and stores one typed phase exit for each of P0 to P5. Production data stays unchanged during P0 to P5. P6 is a separate explicit authorization.

## Refusal codes

A wrong, unreviewed or out-of-order phase exit refuses with the phase-gate code of `MEDULLA-UNIFIED-DATA-PLANE-R026.1`. A difference outside a named deviation refuses the differential run.

## Authoritative types

`GrampsPilotProgress` in `crates/medulla-types` named by `MEDULLA-UNIFIED-DATA-PLANE-R026.1` and `DeviationBaseline` of `MEDULLA-UNIFIED-DATA-PLANE-R028.1`.

## Test vectors

The synthetic fixture tree and the L3 tests of `MEDULLA-UNIFIED-DATA-PLANE-R026.2.2` and `MEDULLA-UNIFIED-DATA-PLANE-R026.3.2` are the vectors.
