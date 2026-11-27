Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__secretsmanager.events-connection-api-key-is-stored-in-secrets-manager-and-rotated events__secretsmanager.events-connection-secret-direct-force-delete-rejected
Issue-Key: secrets-manager|GetSecretValue|ResourceNotFoundException
Issue-Commit: 161f6d1fce0a488dc28b92e49964072868b13e1b

# events__secretsmanager: an EventBridge API_KEY Connection announces a SecretArn but no such secret exists in Secrets Manager, so the credential is unreadable and unprotected (1 cause)

> _Note: this issue was identified against the code base at commit `161f6d1fce0a488dc28b92e49964072868b13e1b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateConnection

## Description
Cause 1 of 1: CreateConnection with AuthorizationType=API_KEY does not materialize the service-linked Secrets Manager secret that holds the key, even though DescribeConnection reports a SecretArn for it. Failing check ids: events__secretsmanager.events-connection-api-key-is-stored-in-secrets-manager-and-rotated, events__secretsmanager.events-connection-secret-direct-force-delete-rejected.

AWS: EventBridge stores a Connection's authorization credential in a Secrets Manager secret that it owns, and reports that secret's ARN on the connection. Smithy (eventbridge): DescribeConnection output member SecretArn, target SecretsManagerSecretArn, @length 20..2048, @pattern ^arn:aws([a-z]|\-)*:secretsmanager:([a-z]|\d|\-)*:([0-9]{12})?:secret:[\/_+=\.@\-A-Za-z0-9]+$, documented as "The ARN of the secret created from the authorization parameters specified for the connection." Because that secret is real, every Secrets Manager read of it succeeds while the connection exists: GetSecretValue(SecretId=<that ARN>) returns a SecretString holding the connection's API key (recorded behavior: a JSON document whose only members are api_key_name and api_key_value, carrying the ApiKeyName and ApiKeyValue the connection was created with), and DescribeSecret(SecretId=<that ARN>) reports OwningService="events" — Smithy (secretsmanager): DescribeSecret output member OwningService, documented as "The ID of the service that created this secret" (service-linked secrets). Because the secret is service-owned, Smithy (secretsmanager) DeleteSecret models InvalidRequestException (httpError 400) and AWS refuses a direct force delete of it with code InvalidRequestException and the message "Operation is not allowed on secret owned by events" — fixed text "Operation is not allowed on secret owned by " followed by the owning service id taken from the secret's state ("events" here). Only DeleteConnection removes the secret, after which GetSecretValue on the ARN answers ResourceNotFoundException with "Secrets Manager can't find the specified secret." and DescribeConnection answers ResourceNotFoundException with "Failed to describe the connection(s). Connection '<name>' does not exist." — fixed text with the connection name taken from the request.

Emulator: CreateConnection is accepted and the connection settles at ConnectionState=AUTHORIZED, and DescribeConnection does return a well-formed secretsmanager SecretArn (both checks got past the ARN-shape and no-credential-echo assertions). But nothing backs that ARN: the very first Secrets Manager call against it, GetSecretValue(SecretId=<the announced SecretArn>), is refused with HTTP 400 ResourceNotFoundException and the message "Secrets Manager can't find the specified secret." Both checks die there — one before it can read the stored key or verify the rotation to a second version via UpdateConnection, the other before it can exercise the DeleteSecret(ForceDeleteWithoutRecovery=True) refusal on an events-owned secret. The ARN is therefore fabricated by the events side and never registered in Secrets Manager state, so DescribeSecret/OwningService, the service-owned delete refusal, the UpdateConnection rotation creating a new version, and DeleteConnection's removal of the secret are all unreachable by construction.

Why: a client that follows the documented contract — take SecretArn off DescribeConnection and read or audit the credential through Secrets Manager — gets ResourceNotFoundException for a secret the control plane just told it exists, so credential retrieval, rotation checks, and any tooling that audits service-linked secrets (DescribeSecret OwningService, version history) all break. Worse, the inverse protection is also absent: because no secret is registered, the refusal that stops a caller from force-deleting an EventBridge-owned credential cannot fire, so an emulated environment cannot reproduce either the readability or the protection of connection credentials.

Fix: the events CreateConnection handler and its connection state — on create it must register a real service-linked secret in Secrets Manager state (SecretString = the {api_key_name, api_key_value} document, OwningService="events") and report that secret's ARN, rather than synthesizing an ARN with no backing entry; UpdateConnection must put a new version of the SAME secret (preserving the prior version id in VersionIdsToStages) and DeleteConnection must remove it. On the Secrets Manager side, DeleteSecret validation must refuse a secret with an OwningService with InvalidRequestException "Operation is not allowed on secret owned by <owning service>" without mutating the secret.

Traces: events__secretsmanager.events-connection-api-key-is-stored-in-secrets-manager-and-rotated (errors at GetSecretValue on the announced SecretArn before the rotation/version assertions); events__secretsmanager.events-connection-secret-direct-force-delete-rejected (errors at the same GetSecretValue in its setup read, before the DeleteSecret force-delete refusal is ever attempted).

## Evidence
Expected (AWS):
- GetSecretValue(SecretId=<DescribeConnection.SecretArn>) -> HTTP 200, SecretString = JSON with exactly the members api_key_name and api_key_value, api_key_name == the ApiKeyName given to CreateConnection, api_key_value == the ApiKeyValue given to CreateConnection.
- DescribeSecret(SecretId=<that ARN>) -> OwningService == "events", DeletedDate absent, exactly one entry in VersionIdsToStages for a freshly created connection.
- UpdateConnection with a new ApiKeyValue -> SecretArn unchanged; the secret advances to the new value and VersionIdsToStages gains a second version while retaining the first.
- DeleteSecret(SecretId=<that ARN>, ForceDeleteWithoutRecovery=True) -> HTTP 400, Error.Code == "InvalidRequestException", Error.Message == "Operation is not allowed on secret owned by events" (fixed text "Operation is not allowed on secret owned by " + the secret's OwningService), with the secret left unchanged and the connection still AUTHORIZED.
- After DeleteConnection -> GetSecretValue settles on HTTP 400 Error.Code == "ResourceNotFoundException", Error.Message == "Secrets Manager can't find the specified secret."

Actual (emulator):
- CreateConnection accepted; DescribeConnection reports ConnectionState="AUTHORIZED" and a syntactically valid secretsmanager SecretArn, and does not echo ApiKeyValue (those assertions passed).
- GetSecretValue(SecretId=<that same SecretArn>) -> HTTP 400 ResourceNotFoundException: "Secrets Manager can't find the specified secret."
  botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the GetSecretValue operation: Secrets Manager can't find the specified secret.
- Consequently DescribeSecret/OwningService, the InvalidRequestException force-delete refusal, the UpdateConnection rotation-to-a-new-version, and the post-DeleteConnection removal are never reached in either check.

## Steps to Reproduce
1. events.create_connection(Name=<conn>, AuthorizationType="API_KEY", AuthParameters={"ApiKeyAuthParameters": {"ApiKeyName": "x-sim2real-api-key", "ApiKeyValue": <key>}}).
2. Poll events.describe_connection(Name=<conn>) until ConnectionState == "AUTHORIZED"; take SecretArn from the response (it is present and well-formed).
3. secretsmanager.get_secret_value(SecretId=<that SecretArn>) -> observe HTTP 400 ResourceNotFoundException "Secrets Manager can't find the specified secret." instead of a SecretString holding {"api_key_name": "x-sim2real-api-key", "api_key_value": <key>}.
4. secretsmanager.describe_secret(SecretId=<that SecretArn>) -> also not found, so OwningService == "events" cannot be observed.
5. secretsmanager.delete_secret(SecretId=<that SecretArn>, ForceDeleteWithoutRecovery=True) -> cannot produce the expected InvalidRequestException "Operation is not allowed on secret owned by events" because no secret is registered.
6. events.update_connection(Name=<conn>, AuthorizationType="API_KEY", AuthParameters={"ApiKeyAuthParameters": {"ApiKeyName": "x-sim2real-api-key", "ApiKeyValue": <key2>}}) then re-read the secret -> the rotation to a second version on the same SecretArn is unobservable.
