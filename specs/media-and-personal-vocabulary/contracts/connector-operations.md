# Seam: Connector operations and ontology cleanups owned by connector repositories

Provider: audio-transcriber CONN-AUDIO-TRANSCRIBER-CONNECTOR-CONTENT-R003
Consumer: medulla MEDULLA-MEDIA-VOCAB-R028

## Wire shape

Rows R028 to R033 name one operation or one ontology cleanup each: transcript list and get, per-segment confidence, citing papers, per-engine result counts, and the removal of the shared-namespace `:Episode` and `:Author` declarations. Other owners: scholarx `CONN-SCHOLARX-CONNECTOR-CONTENT-R001`, searxng-mcp `CONN-SEARXNG-MCP-SEARCH-R001`, arr-mcp `CONN-ARR-MCP-CONNECTOR-CONTENT-R001`, jellyfin-mcp `CONN-JELLYFIN-MCP-CONNECTOR-CONTENT-R001` and audiobookshelf-mcp `CONN-AUDIOBOOKSHELF-MCP-CONNECTOR-CONTENT-R001`. The Medulla consumer row is `MEDULLA-CQ-CONN-R067.9`. Until an operation lands, the Medulla test holds a typed abstention that names the operation and its owner.

## Refusal codes

The typed abstention names the missing operation and its owning repository; the shared decision codes are in `MEDULLA-DECISION-ENGINE-R010.100`.

## Authoritative types

The mapping fixtures under `crates/medulla-core/tests/fixtures/media_vocab/` and each connector's packaged ontology file.

## Test vectors

The served tests named by the rows of this seam in `test-spec.md` hold the vectors that both sides test against.
