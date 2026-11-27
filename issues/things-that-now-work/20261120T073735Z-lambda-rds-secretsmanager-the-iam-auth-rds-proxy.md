Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__rds__secretsmanager.lambda-rds-proxy-missing-permission-denied
Issue-Key: lambda__rds__secretsmanager.lambda-rds-proxy-missing-permission-denied|_invoke#32|Timeout|resp = lam.invoke(FunctionName=name, Payload=json.dumps(payload).encode(), InvocationType="RequestResponse")
Issue-Commit: 27aae757db5fa764a3702a0ead4409204ce3a274

# lambda__rds__secretsmanager: the IAM-auth RDS Proxy permission-boundary scenario no longer reaches any assertion — the check is killed by the 60 s cap

> _Note: this issue was identified against the code base at commit `27aae757db5fa764a3702a0ead4409204ce3a274`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDBProxy

## Description
1 cause, grounded in one failing behavioral check.

Cause 1 of 1: the IAM-auth RDS Proxy permission-boundary scenario makes no observable progress — the check now ends with `Check timed out after 60.0s` instead of reaching its first assertion, so neither the authorized nor the unauthorized rds-db:connect outcome is produced.
Failing check ids for this cause: lambda__rds__secretsmanager.lambda-rds-proxy-missing-permission-denied.

Related open issue, named here because this check is NOT reported as owned by it: 20261117T042331Z-lambda-rds-secretsmanager-an-iam-auth-rds-proxy.md carries the same check id but a different replay key (an AssertionError at run#33 showing pymysql error 2003 from the published proxy endpoint). The failure has since degraded from "the granted role's connection is refused at the transport layer" to "the scenario never converges inside the 60 s cap", so the replay no longer matches that issue's key. The two should be closed together; this entry records the current, un-keyed failure so it is not lost.

AWS: Smithy (rds model): CreateDBProxy/DescribeDBProxies return DBProxy.Endpoint, documented as "The endpoint that you can use to connect to the DB proxy. You include the endpoint value in the connection string for a database client application." Once DBProxy.Status and the registered target's status are `available`, a client inside the proxy's VPC dials that endpoint on the proxy port and completes a MySQL handshake. The IAM-auth decision itself is recorded behavior only (not modeled): a principal holding rds-db:connect on arn:aws:rds-db:<region>:<account>:dbuser:<proxy-resource-id>/<db-user> authenticates with a generate_db_auth_token password and its statement runs (handler payload {"ok": true, "value": 1}); a principal with no such grant is refused by the proxy with MySQL error 1045 (access denied for user) and no statement is executed ({"ok": false, "mysql_error_code": 1045}). Either way the scenario reaches a verdict well inside its budget: the instance, secret, proxy, target registration and two VPC-attached functions all settle and both invocations return.

Emulator: nothing in the scenario converges. The check produces no graded comparison at all and the harness kills it: `Error: Check timed out after 60.0s`. No assertion is reached, so the emulator never produces either documented outcome (a successful handshake for the granted role, or MySQL 1045 for the ungranted one), and the MySQL 2003 transport failure previously recorded for the granted role is no longer even observable within the budget.

Why: an RDS Proxy is the connection path AWS tells serverless clients to use, and rds-db:connect on the dbuser ARN is the only control that separates an authorized from an unauthorized database client. A client relying on AWS's behavior gets neither a working connection nor the 1045 refusal that proves its policy is enforced; an authorization test written against this emulator cannot distinguish "my grant is missing" from "the proxy serves nothing", and the whole permission boundary silently goes unenforced. That the scenario now cannot even be set up inside its budget also hides any later regression.

Fix: the rds crate's DB-proxy plane — the listener behind the address CreateDBProxy/DescribeDBProxies publishes as DBProxy.Endpoint (it must be an address the emulator accepts MySQL connections on from a function in the proxy's VPC), the IAM-auth decision it must make for the token user against rds-db:connect on the dbuser ARN (complete the handshake when granted, answer MySQL 1045 when not), and the control-plane state transitions that must settle promptly enough for CreateDBInstance -> CreateDBProxy -> RegisterDBProxyTargets -> two Lambda invocations to complete inside the check's budget.

Traces: lambda__rds__secretsmanager.lambda-rds-proxy-missing-permission-denied.

Caveat for the fixer: the evidence here is non-convergence rather than a captured wrong response. It is filed because what fails to converge is the emulator's RDS control plane and proxy data plane, not a tool the runner lacked.

## Evidence
Behavioral suite replayed against the emulator built from this checkout. In the lambda__rds__secretsmanager group this check was reported as neither OWNED by an open issue nor blocked by an unimplemented scaffold stub, while its five sibling failures were reported as OWNED.

[mismatch] lambda__rds__secretsmanager.lambda-rds-proxy-missing-permission-denied
  Error: Check timed out after 60.0s

The check (GROUP permission-boundary, SERVICES [lambda, rds, secretsmanager]) creates a MySQL DB instance with an admin credential in Secrets Manager, an RDS Proxy with IAMAuth=REQUIRED in the default VPC's subnets, registers the instance as a target, then invokes two VPC-attached Lambda functions built from one package whose handler is:
  token = boto3.client("rds").generate_db_auth_token(DBHostname=event["proxy_endpoint"], Port=event["port"], DBUsername=event["user"]) ; pymysql.connect(host=..., port=..., user=..., password=token, ssl={...}, connect_timeout=15) ; cur.execute("SELECT 1")
  returning {"ok": True, "value": row[0]} on success and {"ok": False, "mysql_error_code": <code>} on pymysql.err.OperationalError.

Expected (AWS): the role granted rds-db:connect on arn:aws:rds-db:<region>:<account>:dbuser:<proxy-resource-id>/<db-user> returns {"ok": true, "value": 1}; the role without that grant returns {"ok": false, "mysql_error_code": 1045} with no statement executed.
Actual (emulator): no assertion is reached at all — the check is terminated at the 60 s cap with no verdict. The generated instance/proxy/function names, the proxy resource id, the host and port are per-run values and are not requirements.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. Replay the behavioral suite (simparity-behavioral-aws-v1).
3. Run lambda__rds__secretsmanager.lambda-rds-proxy-missing-permission-denied: CreateDBInstance (engine mysql, not publicly accessible) and wait for `available`; store its admin credentials in a Secrets Manager secret; CreateDBProxy (EngineFamily MYSQL, Auth entry with IAMAuth=REQUIRED pointing at that secret) across >=2 subnets; RegisterDBProxyTargets with the instance; wait for proxy and target `available`.
4. Read DescribeDBProxies -> DBProxies[0].Endpoint and derive arn:aws:rds-db:<region>:<account>:dbuser:<proxy-resource-id>/<db-user> from DBProxyArn.
5. Create two VPC-attached Lambda functions from the same package: one role granting rds-db:connect on that exact ARN, one with no such grant.
6. Invoke the granted function -> expected {"ok": true, "value": 1}; invoke the ungranted function -> expected {"ok": false, "mysql_error_code": 1045}.
   Observed: the check never reaches step 6's comparison; the harness terminates it with `Check timed out after 60.0s`.

## Resolution

FIXED — the scenario now converges and both documented outcomes are produced: the role granted
`rds-db:connect` on the proxy's `dbuser` resource authenticates through the IAM-auth proxy and its
statement runs, and the role without that grant is refused by the proxy with MySQL 1045 and runs no
statement.

What did not converge was the invocation, in three places, each a general rule rather than this
scenario's shape:

1. An authentication token is PRESIGNED in the client over the address the caller named — a SigV4
   canonical request carries the path, the query, the headers and the payload hash, and the address
   only as the `host` header — so the SDK neither resolves nor parses `DBHostname`: whatever a caller
   read out of a control plane's endpoint member is minted a token addressed `<DBHostname>:<Port>`.
   The runtime's SDK seam was composing that authority into the URI it handed the signer, so a value
   carrying a port of its own — what a member that models no port publishes — was refused as a
   malformed URL and the handler raised instead of being handed a token.
2. The addresses one proxy endpoint answers at are both readings of the value it published — the
   host alone and the whole authority — each completed with either port that endpoint answers at, so
   a token minted for any of them is a token for that one endpoint.
3. `UserAuthConfig$UserName` is optional and the credential the proxy logs in with is the one in the
   secret the same entry names, so an entry that spells no user out is in force for the user that
   secret holds — the proxy created the way the service documents one, with the scheme, the secret
   and the IAM mode and no user. The secret is read through Secrets Manager's own operation over the
   in-process callback, and only the user is taken from it.
