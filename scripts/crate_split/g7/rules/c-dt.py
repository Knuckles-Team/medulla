RANK={'medulla-decision-types':1,'medulla-decision-outcome-types':2}
W=['policy_evolution','current_plan']+['decision::'+x for x in 'answer_ranking concentration context_budget entry_trust evolution_round defect_triage limit_guard red_team web_fetch_selection topology_tests execution_answer rapid_r044 recommend load_set ingestion_ladder rapid_r056 prompt_candidate usage_guidance branch_outcomes promotion coverage skill_upgrade derivation'.split()]
RULES=[(x,'medulla-decision-outcome-types') for x in W]
DEFAULT='medulla-decision-types'
