RANK={'medulla-persistence':1,'medulla-persistence-records':2,'medulla-persistence-ops':3}
RULES=[(x,'medulla-persistence-records') for x in ['agent_*','decision_record','decision_jobs','connector_pack','write_back','guardrail_rule','fleet_components']]+[(x,'medulla-persistence-ops') for x in ['backup','online_reshard','shard_migrate','store_upgrade']]
DEFAULT='medulla-persistence'
