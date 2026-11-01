RANK={'medulla-work-types':1,'medulla-agent-types':2}
W=['work_*','orchestration','control_lease','jobs','worker_registry','action_spec','native_control','memory','search_task','run_*','host_start','agent_fixtures::host_start']
RULES=[(x,'medulla-work-types') for x in W]
DEFAULT='medulla-agent-types'
