RANK={'medulla-sql-lang':1,'medulla-sql':2,'medulla-sql-tests':3}
RULES=[('sql::'+x,'medulla-sql-lang') for x in ['classify','pgq','plpgsql','pgfamily','pgvector_ddl','funcs','typed_result']]+[('@tests','medulla-sql-tests')]
DEFAULT='medulla-sql'
