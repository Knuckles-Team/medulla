RANK={'medulla-server-obs':1,'medulla-server-lake':1,'medulla-server-graph':2,'medulla-server-consensus':3,'medulla-server-identity':3,'medulla-server-router':4,'medulla-server':5,
'medulla-server-cluster-tests':6,'medulla-server-store-tests':6,'medulla-server-served-tests':6}
RULES=[(x,'medulla-server-obs') for x in ['obs','traces','promql','qos']]+[(x,'medulla-server-lake') for x in ['lake','graphql_sub','policy_export']]
RULES+=[(x,'medulla-server-graph') for x in ['dispatch::shared','dispatch::graph_pipeline','dispatch::change_envelope','dispatch::change_envelope_draft','dispatch::policy_evolution','dispatch::sparql_update','dispatch::request_boundary::preflight','dispatch::request_boundary::screen','semantic_activation','handlers']]
RULES+=[('dispatch::consensus','medulla-server-consensus')]+[(x,'medulla-server-identity') for x in ['dispatch::identity_store','dispatch::elevation','dispatch::governed']]
RULES+=[(x,'medulla-server-router') for x in ['dispatch::router','dispatch::request_boundary','dispatch::request_boundary::*','dispatch::telemetry','dispatch::test_support','dispatch::seam','dispatch::dispatch_e2e_tests']]
RULES+=[('split_tests::'+x,'medulla-server-cluster-tests') for x in ['raft_cluster','raft_command','xshard_harness','placement_harness','resource_acceptance_test']]
RULES+=[('split_tests::'+x,'medulla-server-store-tests') for x in ['admin_agent_library_security','server_api_saga','store_run_owner','store_terminal_batch','impact_watch','mutation_batch_internal_replay','mutation_batch_lifecycle','mutation_redb','persistence_txn','server_api_mutation_inventory']]
RULES+=[('split_tests::medulla_*','medulla-server-served-tests'),('split_tests::wired_edge_index','medulla-server-served-tests'),('tests','medulla-server-served-tests')]
DEFAULT='medulla-server'
