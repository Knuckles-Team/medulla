RANK={'medulla-server-api':1,'medulla-server-auth':2,'medulla-server-cdc':3,'medulla-server-mutation':4,'medulla-server-blob':5,'medulla-server-ops':6}
RULES=[(x,'medulla-server-auth') for x in ['auth','access','sql_catalog_acl','sql_tables']]+[(x,'medulla-server-cdc') for x in ['cdc','cep','matview','reasoning_cascade','impact_watch','cache_coherence']]+[(x,'medulla-server-mutation') for x in ['mutation','mutation_batch']]+[(x,'medulla-server-blob') for x in ['blob','kv','sparql_http']]+[(x,'medulla-server-ops') for x in ['api::admin','api::ingest','api::query_txn']]
DEFAULT='medulla-server-api'
CUTS={'auth::VerifiedRequestContext':'medulla-server-api'}  # item and handle cuts live in cuts.json
