RANK={'medulla-tables':1,'medulla-table-store':2,'medulla-sql-udfs':3,'medulla-sql':4,'medulla-cypher':4}
RULES=[('tables::schema','medulla-tables'),('tables::index','medulla-tables'),('tables::migration','medulla-tables'),
('tables::provider','medulla-sql'),('tables*','medulla-table-store'),('plan_scope','medulla-table-store'),
('sql::udfs','medulla-sql-udfs'),('sql::numeric','medulla-sql-udfs'),('sql::sketch_udfs','medulla-sql-udfs'),('sql::series_udwf','medulla-sql-udfs'),('sql::tablefuncs','medulla-sql-udfs'),
('sql*','medulla-sql'),('cypher','medulla-cypher'),('edge_index','medulla-cypher')]
DEFAULT='medulla-sql'
CUTS={f'sql::pgfamily::{x}':'medulla-table-store' for x in ['VectorMetric','AnnMethod','AnnIndexPlan','HypertablePlan','*']}
CUTS.update({f'sql::ann::{x}':'medulla-table-store' for x in ['metric_to_ann','isqrt']})
CUTS.update({f'sql::{x}':'medulla-table-store' for x in ['VectorMetric','AnnMethod','AnnIndexPlan','HypertablePlan','metric_to_ann','isqrt','DropAnnIndex']})
CUTS.update({'sql::exec_sql_typed_with_tables':'medulla-sql'})
