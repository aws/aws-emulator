Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync__rds.mysql-not-filter-negates-its-anded-conditions appsync__rds.rds-utils-unsupported-where-operator-rejected-row-unchanged appsync__rds.rds-utils-where-operators-and-returning-on-aurora-postgres appsync__rds.mysql-insert-then-select-duplicate-key-rejected
Issue-Key: appsync__rds.mysql-not-filter-negates-its-anded-conditions|run#29|AssertionError|assert both == {"data": {"excluding": kept}}, (
Issue-Key: appsync__rds.rds-utils-unsupported-where-operator-rejected-row-unchanged|run#41|AssertionError|assert json.loads(ok["data"]["renameBoat"]) == [{"id": 1, "name": "kestrel-ii"}], (
Issue-Key: appsync__rds.rds-utils-where-operators-and-returning-on-aurora-postgres|run#49|AssertionError|assert everything == seeded, (
Issue-Key: appsync__rds.mysql-insert-then-select-duplicate-key-rejected|run#30|AssertionError|assert errors == [(["enroll"], "400 Bad Request", f"RDSHttp:{duplicate}")], (
Issue-Commit: 85a8ef40f44f446d2f7f13d4131def4b1abce9f7

# appsync__rds: 4 net-new causes — NOT filter rejected, unsupported where-operator, null columns returned for an empty column list, duplicate-key error text

> _Note: this issue was identified against the code base at commit `85a8ef40f44f446d2f7f13d4131def4b1abce9f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteStatement

## Description
Four distinct causes in the RDS (Aurora Serverless / rds-utils) resolver surface; each must be closed before submitting.

Cause 1 of 4: a NOT filter over ANDed conditions is refused instead of negated.
AWS: rds-utils accepts a `not` filter whose value is a LIST of condition objects ([{"name":{"eq":"ada"}},{"id":{"ge":2}}]) and negates the conjunction, so the select returns the complementary rows (ids 1, 2 and 4 in the recorded fixture).
Emulator: nulls the field and reports 'A condition is an object naming the columns it compares, and this one is [{"name":{"eq":"ada"}},{"id":{"ge":2}}].' — it only accepts an object, so the documented list form of `not` is rejected.
Why: a query AWS answers with rows returns an error here, and the resolver's data is null.
Fix: the rds-utils where-clause builder's handling of `not` over a list of conditions.
Traces: appsync__rds.mysql-not-filter-negates-its-anded-conditions

Cause 2 of 4: an unsupported where operator is not refused — it silently matches nothing.
AWS: a where clause naming an operator rds-utils does not support (beginsWith) is REFUSED, and the row it would have touched is left unchanged.
Emulator: the statement runs and returns an empty result ('[]') instead of a refusal, so the caller cannot tell an unsupported operator from a query that matched no rows (recorded behavior only).
Why: a typo'd or unsupported operator looks like "no matching rows", so updates silently do nothing.
Fix: operator validation in the rds-utils where-clause builder.
Traces: appsync__rds.rds-utils-unsupported-where-operator-rejected-row-unchanged

Cause 3 of 4: a select with no column list returns NULL columns AWS omits.
AWS: a select naming no columns returns every column of each row, but a column whose value is NULL is ABSENT from that row's map — e.g. {'id': 2, 'name': 'antares', 'mag': 10} with no 'note' member.
Emulator: returns the member with a null value ({'id': 2, 'name': 'antares', 'mag': 10, 'note': None}).
Why: a client distinguishing "column absent" from "column null" (GraphQL nullability, partial updates) sees a different row shape and may write nulls back.
Fix: the rds-data result-to-record mapping used by the RDS resolver (drop NULL-valued columns).
Traces: appsync__rds.rds-utils-where-operators-and-returning-on-aurora-postgres

Cause 4 of 4: the duplicate-key error is reported with MySQL's raw text rather than AWS's error shape.
AWS: a duplicate-key insert through the RDS resolver reports a RDSHttp:DatabaseErrorException whose message is AWS's rendering of the database error for this class; the recorded expectation differs from the emulator's text below.
Emulator: reports [(['enroll'], 'RDSHttp:DatabaseErrorException', "Duplicate entry '7' for key 'members.PRIMARY'; Error code: 1062; SQLState: 23000")] — the errorType is right, the message is the engine's string with the emulator's own "; Error code: ...; SQLState: ..." tail, and the key name is rendered as 'members.PRIMARY'.
Why: clients match on the message to detect a conflict and retry; a different rendering breaks that.
Fix: the RDS resolver's database-error-to-GraphQL-error message mapping.
Traces: appsync__rds.mysql-insert-then-select-duplicate-key-rejected

## Evidence
Cause 1: "NOT(name = 'ada' AND id >= 2) returned {'data': {'excluding': None}, 'errors': [{'path': ['excluding'], 'message': 'A condition is an object naming the columns it compares, and this one is [{\"name\":{\"eq\":\"ada\"}},{\"id\":{\"ge\":2}}].'}]}, expected rows [1, 2, 4]".
Cause 2: 'the beginsWith rename returned \'[]\''.
Cause 3: 'select with no columns returned [... {\'id\': 2, \'name\': \'antares\', \'mag\': 10, \'note\': None} ...]; expected every column of [... {\'id\': 2, \'name\': \'antares\', \'mag\': 10} ...]'.
Cause 4: 'the duplicate-key insert reported [([\'enroll\'], \'RDSHttp:DatabaseErrorException\', "Duplicate entry \'7\' for key \'members.PRIMARY\'; Error code: 1062; SQLState: 23000")]'.

## Steps to Reproduce
Create an RDS (Aurora) data source and resolvers using the rds-utils helpers, then: (1) select with a `not` filter whose value is a list of two condition objects; (2) run an update whose where clause uses the unsupported beginsWith operator; (3) select with an empty column list over a table holding NULLs; (4) insert a row whose primary key already exists. Replay the checks in trace_ids.
