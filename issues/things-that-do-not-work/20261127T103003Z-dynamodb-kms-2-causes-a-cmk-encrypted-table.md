Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__kms.cmk-encrypted-table-holds-a-dynamodb-grant-scoped-to-the-table dynamodb__kms.create-table-sse-unknown-alias-arn-rejected
Issue-Key: dynamodb__kms.cmk-encrypted-table-holds-a-dynamodb-grant-scoped-to-the-table|run#16|AssertionError|assert scoped, (
Issue-Key: dynamodb__kms.create-table-sse-unknown-alias-arn-rejected|run#16|AssertionError|assert (code, relayed) == ("ValidationException", expected), (
Issue-Commit: 8133ebe260b7cb1c7e11e2995dae089129b7fabf

# dynamodb__kms: 2 causes — a CMK-encrypted table mints no KMS grant, and the unknown-alias refusal relays the wrong KMS message

> _Note: this issue was identified against the code base at commit `8133ebe260b7cb1c7e11e2995dae089129b7fabf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTable

## Description
Two distinct root causes, each grounded in a failing behavioral check.

Cause 1 of 2: a table encrypted with a customer managed key holds no KMS grant constrained to it.
  AWS: recorded behavior only. When a table becomes ACTIVE under an SSESpecification naming a customer managed key, DynamoDB has created a grant on that key for the dynamodb.amazonaws.com service principal, constrained to the table (an encryption-context-subset constraint carrying the table ARN); ListGrants on the key returns it.
  Emulator: ListGrants on the key returns [] for the ACTIVE table.
  Why: a client auditing which services hold grants on its CMK, or revoking a grant to cut a table off from its key, sees nothing to audit or revoke — the key/table binding that AWS makes visible does not exist.
  Fix: the dynamodb CreateTable handler's SSE path (create the service grant on the key, constrained to the table) and the KMS grant store it writes into.
  Traces: dynamodb__kms.cmk-encrypted-table-holds-a-dynamodb-grant-scoped-to-the-table.

Cause 2 of 2: CreateTable naming an alias ARN that no alias holds relays the wrong KMS message.
  AWS: recorded behavior only for the relay shape; the KMS refusal itself is an alias-not-found. CreateTable is refused 400 ValidationException whose message is DynamoDB's relay of KMS's answer: 'KMS validation error: com.amazonaws.services.kms.model.NotFoundException: Alias <alias arn> is not found. (Service: AWSKMS; Status Code: 400; Error Code: NotFoundException; Request ID: <id>; Proxy: null)' — fixed text with <alias arn> from the request and <id> per call.
  Emulator: same code, but the relayed KMS message is the key-not-found wording: "Key '<alias arn>' does not exist" instead of 'Alias <alias arn> is not found.'
  Why: a client parsing the relayed KMS error (or an operator reading it) is told the key does not exist when the real fault is that the alias does not exist; the two are distinct KMS messages.
  Fix: KMS's alias resolution error construction (the alias-not-found message) as surfaced through the dynamodb CreateTable SSE key-resolution relay.
  Traces: dynamodb__kms.create-table-sse-unknown-alias-arn-rejected.

## Evidence
dynamodb__kms.cmk-encrypted-table-holds-a-dynamodb-grant-scoped-to-the-table —
AssertionError: the ACTIVE table 'sim2real-behav-ddbgrant-<id>' encrypted with <key arn> left no grant constrained to it on the key; ListGrants returned [].

dynamodb__kms.create-table-sse-unknown-alias-arn-rejected —
AssertionError: CreateTable naming an alias ARN no alias holds gave 'ValidationException': "KMS validation error: com.amazonaws.services.kms.model.NotFoundException: Key '<alias arn>' does not exist (Service: AWSKMS; Status Code: 400; Error Code: NotFoundException; Request ID: <id>; Proxy: null)".
Expected the same code with 'Alias <alias arn> is not found.' as the relayed KMS message.

## Steps to Reproduce
1. Cause 1: create a symmetric CMK; dynamodb.create_table(..., SSESpecification={'Enabled':True,'SSEType':'KMS','KMSMasterKeyId':<key id>}); wait for ACTIVE; kms.list_grants(KeyId=<key>) -> expect one dynamodb grant constrained to the table ARN; observe [].
2. Cause 2: build an alias ARN in the account/Region that no alias holds; dynamodb.create_table(..., KMSMasterKeyId=<that alias arn>) -> expect ValidationException relaying 'Alias <alias arn> is not found.'; observe the "Key '<alias arn>' does not exist" wording.
