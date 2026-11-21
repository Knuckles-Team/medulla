SELECT n.nspname, c.relname, CASE c.relkind WHEN 'r' THEN 'table' WHEN 'v' THEN 'view' ELSE 'other' END AS type, pg_catalog.pg_get_userbyid(c.relowner) AS owner FROM pg_catalog.pg_class c LEFT JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace WHERE c.relkind IN ('r', 'v') AND n.nspname <> 'pg_catalog' AND n.nspname <> 'information_schema' AND pg_catalog.pg_table_is_visible(c.oid) ORDER BY 1, 2
SELECT c.oid, n.nspname, c.relname FROM pg_catalog.pg_class c LEFT JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace WHERE c.relname = 't_visible' AND pg_catalog.pg_table_is_visible(c.oid) ORDER BY 2, 3
SELECT c.relkind, c.relpersistence, c.relhasindex, c.relnatts, c.reltuples FROM pg_catalog.pg_class c WHERE c.relname = 't_visible'
SELECT a.attname, pg_catalog.format_type(a.atttypid, a.atttypmod), a.attnotnull, a.attnum FROM pg_catalog.pg_attribute a JOIN pg_catalog.pg_class c ON a.attrelid = c.oid WHERE c.relname = 't_visible' AND a.attnum > 0 AND NOT a.attisdropped ORDER BY a.attnum
SELECT i.indexrelid, i.indisprimary, i.indisunique, i.indnatts FROM pg_catalog.pg_index i JOIN pg_catalog.pg_class c ON c.oid = i.indrelid WHERE c.relname = 't_visible' ORDER BY i.indisprimary DESC, i.indexrelid
SELECT con.conname, con.contype, pg_catalog.pg_get_constraintdef(con.oid) FROM pg_catalog.pg_constraint con JOIN pg_catalog.pg_class c ON c.oid = con.conrelid WHERE c.relname = 't_visible' ORDER BY 1
SELECT n.nspname, c.relname, pg_catalog.pg_get_userbyid(c.relowner) FROM pg_catalog.pg_class c LEFT JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace WHERE c.relkind = 'r' AND n.nspname = 'public' AND pg_catalog.pg_table_is_visible(c.oid) ORDER BY 1, 2
SELECT table_schema, table_name, table_type FROM information_schema.tables WHERE table_type = 'BASE TABLE' AND table_schema = 'public' ORDER BY 1, 2
SELECT count(*) FROM pg_catalog.pg_class c WHERE c.relkind = 'r' AND pg_catalog.pg_table_is_visible(c.oid)
SELECT c.relname, c.reltuples FROM pg_catalog.pg_class c WHERE c.relkind = 'r' ORDER BY c.relname
SELECT n.nspname AS name, pg_catalog.pg_get_userbyid(n.nspowner) AS owner FROM pg_catalog.pg_namespace n WHERE n.nspname NOT LIKE 'pg_%' AND n.nspname <> 'information_schema' ORDER BY 1
SELECT catalog_name, schema_name, schema_owner FROM information_schema.schemata ORDER BY schema_name
SELECT oid, nspname FROM pg_catalog.pg_namespace ORDER BY nspname
SELECT n.nspname, p.proname, p.pronargs, p.prokind FROM pg_catalog.pg_proc p LEFT JOIN pg_catalog.pg_namespace n ON n.oid = p.pronamespace ORDER BY 1, 2
SELECT routine_schema, routine_name, routine_type, data_type FROM information_schema.routines ORDER BY 1, 2
SELECT p.proname, pg_catalog.format_type(p.prorettype, NULL) FROM pg_catalog.pg_proc p WHERE p.proretset = false ORDER BY 1
SELECT ic.relname, tc.relname, i.indisunique, i.indisprimary FROM pg_catalog.pg_index i JOIN pg_catalog.pg_class ic ON ic.oid = i.indexrelid JOIN pg_catalog.pg_class tc ON tc.oid = i.indrelid ORDER BY 1, 2
SELECT count(*) FROM pg_catalog.pg_index i
SELECT c.relname FROM pg_catalog.pg_class c WHERE c.relkind = 'i' AND pg_catalog.pg_table_is_visible(c.oid) ORDER BY 1
SELECT i.indrelid, i.indkey, i.indnatts FROM pg_catalog.pg_index i ORDER BY i.indrelid, i.indexrelid
SELECT n.nspname, c.relname, pg_catalog.pg_get_userbyid(c.relowner) FROM pg_catalog.pg_class c LEFT JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace WHERE c.relkind = 'v' AND pg_catalog.pg_table_is_visible(c.oid) ORDER BY 1, 2
SELECT table_schema, table_name, view_definition FROM information_schema.views WHERE table_schema = 'public' ORDER BY 1, 2
SELECT count(*) FROM pg_catalog.pg_class c WHERE c.relkind = 'v'
SELECT d.datname, pg_catalog.pg_get_userbyid(d.datdba), d.encoding, d.datcollate, d.datctype FROM pg_catalog.pg_database d ORDER BY 1
SELECT d.datname FROM pg_catalog.pg_database d WHERE d.datallowconn AND NOT d.datistemplate ORDER BY 1
SELECT oid, datname, encoding FROM pg_catalog.pg_database ORDER BY oid
SELECT c.oid, n.nspname, c.relname, c.relkind, c.relhasindex, pg_catalog.obj_description(c.oid, 'pg_class') FROM pg_catalog.pg_class c LEFT JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace WHERE c.relname = 't_visible' AND pg_catalog.pg_table_is_visible(c.oid)
SELECT a.attname, pg_catalog.format_type(a.atttypid, a.atttypmod), a.attnotnull, a.atthasdef, pg_catalog.col_description(a.attrelid, a.attnum) FROM pg_catalog.pg_attribute a JOIN pg_catalog.pg_class c ON a.attrelid = c.oid WHERE c.relname = 't_visible' AND a.attnum > 0 AND NOT a.attisdropped ORDER BY a.attnum
SELECT c2.relname, i.indisprimary, i.indisunique, pg_catalog.pg_get_indexdef(i.indexrelid) FROM pg_catalog.pg_index i JOIN pg_catalog.pg_class c ON c.oid = i.indrelid JOIN pg_catalog.pg_class c2 ON c2.oid = i.indexrelid WHERE c.relname = 't_visible' ORDER BY i.indisprimary DESC, c2.relname
SELECT con.conname, pg_catalog.pg_get_constraintdef(con.oid), con.condeferrable, con.condeferred FROM pg_catalog.pg_constraint con JOIN pg_catalog.pg_class c ON c.oid = con.conrelid WHERE c.relname = 't_visible' ORDER BY 1
SELECT d.objid, d.refobjid, d.deptype FROM pg_catalog.pg_depend d JOIN pg_catalog.pg_class c ON c.oid = d.refobjid WHERE c.relname = 't_visible' ORDER BY 1
SELECT ds.objoid, ds.description FROM pg_catalog.pg_description ds JOIN pg_catalog.pg_class c ON c.oid = ds.objoid WHERE c.relname = 't_visible' ORDER BY ds.objsubid
SELECT version()
SELECT current_schema()
SELECT current_database()
SELECT current_user
SELECT session_user
SELECT setting FROM pg_catalog.pg_settings WHERE name = 'server_version'
SELECT setting FROM pg_catalog.pg_settings WHERE name = 'TimeZone'
SELECT setting FROM pg_catalog.pg_settings WHERE name = 'standard_conforming_strings'
