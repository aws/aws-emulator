Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync__rds__rds-data.rds-select-utility-schema-qualified-table-reads-that-schema appsync__rds__rds-data.rds-select-utility-table-qualified-column-names-the-from-table appsync__rds__rds-data.rds-select-utility-contains-with-a-number-operand-rejected appsync__rds__rds-data.rds-insert-utility-quotes-mixed-case-columns-and-binds-values
Issue-Key: appsync__rds__rds-data|ExecuteStatement|DatabaseErrorException
Issue-Key: appsync__rds__rds-data.rds-select-utility-table-qualified-column-names-the-from-table|run#9|AssertionError|assert errors == [(
Issue-Key: appsync__rds__rds-data.rds-select-utility-contains-with-a-number-operand-rejected|run#7|AssertionError|assert bad.get("data") == {"labelsContainingNumber": None}, f"contains with a number returned {bad!r}"
Issue-Key: appsync__rds__rds-data.rds-insert-utility-quotes-mixed-case-columns-and-binds-values|run#7|TypeError|assert row["ownerName"] == HOSTILE and row["rank"] == 7, f"RETURNING row was {row!r}"
Issue-Commit: 85a8ef40f44f446d2f7f13d4131def4b1abce9f7

# appsync__rds__rds-data: 4 net-new causes — CREATE SCHEMA unsupported, schema/table-qualified names mishandled, contains with a number accepted, insert utility returns nothing

> _Note: this issue was identified against the code base at commit `85a8ef40f44f446d2f7f13d4131def4b1abce9f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteStatement

## Description
Four distinct causes where the rds-data backing store behind the AppSync RDS resolver diverges from the Aurora engine AWS runs; each must be closed before submitting.

Cause 1 of 4: CREATE SCHEMA is not supported, so schema-qualified tables cannot exist.
AWS: ExecuteStatement with "CREATE SCHEMA vault" succeeds on Aurora PostgreSQL, and a select naming a schema-qualified table (vault.<table>) reads that schema.
Emulator: ExecuteStatement raises DatabaseErrorException 'near "SCHEMA": syntax error in CREATE SCHEMA vault at offset 7' — the SQL dialect behind rds-data does not implement schemas at all (the message is SQLite's).
Why: any application using a non-default schema cannot be set up, and the error text names a dialect AWS never exposes.
Fix: the rds-data statement execution backend — support CREATE SCHEMA and schema-qualified resolution (or translate it).
Traces: appsync__rds__rds-data.rds-select-utility-schema-qualified-table-reads-that-schema

Cause 2 of 4: a table-qualified column name is treated as one quoted identifier.
AWS: a column qualified by a table that is not in the FROM clause is refused by PostgreSQL with the missing-table diagnosis for the qualifier; the qualified name is parsed as <table>.<column>, not as a single column called "widgets.label".
Emulator: answers [('RDSHttp:DatabaseErrorException', 'ERROR: column "widgets.label" does not exist; SQLState: 42703')] — it quotes the whole qualified name as one column, so the diagnosis names the wrong object.
Why: the client is told a column does not exist when the real fault is the table qualifier, so the query is debugged in the wrong place.
Fix: qualified-identifier handling in the rds-data SQL layer / the rds-utils select builder.
Traces: appsync__rds__rds-data.rds-select-utility-table-qualified-column-names-the-from-table

Cause 3 of 4: `contains` with a number operand is accepted instead of refused.
AWS: the rds-utils `contains` operator takes a string operand; given a number it is REFUSED and the field does not resolve to rows.
Emulator: runs the comparison and answers rows — {'data': {'labelsContainingNumber': [{'id': 1, 'label': 'gadget-5'}]}} (recorded behavior only).
Why: a type error in a filter silently returns data here and fails in production.
Fix: operand type validation for `contains` in the rds-utils where-clause builder.
Traces: appsync__rds__rds-data.rds-select-utility-contains-with-a-number-operand-rejected

Cause 4 of 4: the insert utility answers nothing for mixed-case columns.
AWS: the rds-utils insert helper quotes mixed-case column names and binds the values, and the resolver answers the inserted record.
Emulator: the resolver's result is absent — the check fails with TypeError: 'NoneType' object is not subscriptable when reading the returned record, so no row/record came back.
Why: an insert through the documented helper returns nothing, so the client cannot read back what it wrote.
Fix: the rds-utils insert statement builder (identifier quoting and the RETURNING/record path) in the RDS resolver.
Traces: appsync__rds__rds-data.rds-insert-utility-quotes-mixed-case-columns-and-binds-values

## Evidence
Cause 1: botocore.errorfactory.DatabaseErrorException: An error occurred (DatabaseErrorException) when calling the ExecuteStatement operation: near "SCHEMA": syntax error in CREATE SCHEMA vault at offset 7.
Cause 2: 'a column qualified by an absent table was refused with [(\'RDSHttp:DatabaseErrorException\', \'ERROR: column "widgets.label" does not exist; SQLState: 42703\')]'.
Cause 3: 'contains with a number returned {\'data\': {\'labelsContainingNumber\': [{\'id\': 1, \'label\': \'gadget-5\'}]}}'.
Cause 4: TypeError: 'NoneType' object is not subscriptable.

## Steps to Reproduce
Against an RDS data source reached through rds-data: (1) ExecuteStatement 'CREATE SCHEMA vault' and then select from vault.<table>; (2) select a column qualified by a table absent from the FROM clause; (3) use the rds-utils select helper with a `contains` filter whose operand is a number; (4) use the rds-utils insert helper on a table with mixed-case column names and read the returned record. Replay the checks in trace_ids.
