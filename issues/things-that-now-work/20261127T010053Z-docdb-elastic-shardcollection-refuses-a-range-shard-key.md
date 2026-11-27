Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: docdb-elastic.mongo-elastic-shard-key-must-be-one-hashed-field-present-in-every-write
Issue-Shape-Id: com.amazonaws.docdbelastic#ChimeraDbLionfishServiceLambda
Issue-Key: docdb-elastic.mongo-elastic-shard-key-must-be-one-hashed-field-present-in-every-write|run#5|AssertionError|assert (ranged.get("ok"), ranged.get("code"), ranged.get("errmsg")) == (
Issue-Commit: 82aaf2cb51feda9d788e7c4e8f0e80ec9e3e7eed

# docdb-elastic: shardCollection refuses a range shard key with the wrong errmsg/codeName (MongoDB wire data plane)

> _Note: this issue was identified against the code base at commit `82aaf2cb51feda9d788e7c4e8f0e80ec9e3e7eed`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
shardCollection

## Description
One net-new cause.

Cause 1 of 1: over the MongoDB wire protocol, an Amazon DocumentDB *elastic* cluster refuses `shardCollection` on a range (non-hashed) shard key with its own elastic-specific command error, not the community-MongoDB one. The emulator answers with a community-style message and codeName, so the refusal is distinguishable from real elastic clusters even though the numeric code matches.
Failing check: docdb-elastic.mongo-elastic-shard-key-must-be-one-hashed-field-present-in-every-write

AWS: recorded behavior only — this is the native MongoDB wire protocol of a DocumentDB elastic cluster, not a Smithy-modelled operation (smithy_query over the docdb-elastic model covers only the control plane: CreateCluster/GetCluster/DeleteCluster etc., and models nothing about the data plane's command replies). The recording shows `{"shardCollection": "<db>.<coll>", "key": {"<field>": 1}}` answered as a command failure: `ok: 0`, `code: 61`, and `errmsg` exactly `Create collection failed. Shard Key type must be hashed not range based`. That errmsg is entirely FIXED text for this error class — it interpolates no part of the request (no database, collection or field name), which is precisely what distinguishes the elastic wording from community MongoDB's. The recording does not show a `codeName` for this reply. The same class of elastic refusals in the recording also includes `Create collection failed. Must have exactly one Shard Key` (code 61) for a compound/two-field key, and `a collection '<db>.<coll>' already exists` (code 48) for re-sharding an existing collection, where `<db>.<coll>` are taken from the request's namespace.

Emulator: the reply is `{'ok': 0.0, 'errmsg': 'Shard key { k: 1 } must be hashed', 'code': 61, 'codeName': 'ShardKeyNotFound'}`. The numeric `code` 61 is right, but the `errmsg` is a community-MongoDB style message that echoes the requested key document (`{ k: 1 }` — a value taken from the request, not part of any AWS message template), and an extra `codeName: 'ShardKeyNotFound'` is stamped on, which the recorded elastic reply does not carry.

Why: a client that keys off the refusal text — an ODM, a migration tool, or test code asserting an elastic cluster rejected its range key — sees a message that never occurs on a real elastic cluster and does not see the elastic-specific prefix `Create collection failed.` that tells it the collection was not created rather than that a shard key lookup failed. Code 61 (`ShardKeyNotFound`) in community MongoDB means "the shard key was absent from a document", so a client reading `codeName` plus this errmsg concludes a write-targeting problem instead of an unsupported-shard-key-type problem at collection-creation time, and will retry rather than correct the key definition. Because this check aborts at the first assertion, every later elastic shard-key rule in the same flow (one-shard-key-only, re-shard refusal code 48, keyless insert / non-targetable upsert / shard-key update write errors 61/61/20) is still unverified against the emulator and may diverge too.

Fix: the MongoDB wire data plane's command dispatch for `shardCollection` on an elastic cluster — its shard-key validation and the error it builds. It should emit the elastic command-error wording (fixed text, no request values interpolated) for a non-hashed key and a multi-field key, and must not add a community `codeName` the recorded reply omits. The elastic data plane needs its own validation path here rather than reusing the community/docdb shard-key error builder.

Traces: docdb-elastic.mongo-elastic-shard-key-must-be-one-hashed-field-present-in-every-write (asserts the range-key refusal, then the compound-key refusal, then the full sharded-write flow).

## Evidence
Check: docdb-elastic.mongo-elastic-shard-key-must-be-one-hashed-field-present-in-every-write
Request (OP_MSG, authenticated SCRAM-SHA-1 session to the ACTIVE cluster's clusterEndpoint:27017):
  {"shardCollection": "sim2real.ranged", "key": {"k": 1}, "$db": "admin"}

Expected (recorded AWS elastic reply):
  ok    = 0
  code  = 61
  errmsg = "Create collection failed. Shard Key type must be hashed not range based"   <- fixed text, no request values
  (no codeName)

Actual (emulator):
  {'ok': 0.0, 'errmsg': 'Shard key { k: 1 } must be hashed', 'code': 61, 'codeName': 'ShardKeyNotFound'}

AssertionError: expected a range shard key to be refused with code 61, got {'ok': 0.0, 'errmsg': 'Shard key { k: 1 } must be hashed', 'code': 61, 'codeName': 'ShardKeyNotFound'}
  at check_docdb_elastic_mongo_elastic_shard_key_must_be_one_hashed_field_present_in_every_write.py:36

Diff: `code` matches (61); `errmsg` is community-MongoDB wording that interpolates the requested key document instead of the fixed elastic text beginning "Create collection failed."; `codeName` is present when the recorded reply has none.

## Steps to Reproduce
1. Create a docdb-elastic cluster (CreateCluster: authType PLAIN_TEXT, shardCapacity 2, shardCount 1, shardInstanceCount 1, two private subnets, one SG admitting 27017 from inside the VPC) and poll GetCluster until status == ACTIVE.
2. From inside the VPC, open a TLS socket to the cluster's clusterEndpoint on port 27017 and authenticate as the admin user with SCRAM-SHA-1 against the admin database.
3. Send OP_MSG command: {"shardCollection": "sim2real.ranged", "key": {"k": 1}, "$db": "admin"}.
4. Observe the reply: expected ok=0, code=61, errmsg="Create collection failed. Shard Key type must be hashed not range based" and no codeName; the emulator returns errmsg "Shard key { k: 1 } must be hashed" with codeName "ShardKeyNotFound".
5. (Blocked by step 4 today) continue the flow to confirm the remaining elastic shard-key rules: a two-field key must be refused code 61 "Create collection failed. Must have exactly one Shard Key"; a hashed single-field key must succeed; re-sharding must be refused code 48 "a collection '<db>.<coll>' already exists"; then a keyless insert, a non-targetable upsert and a shard-key update must each come back as one writeError with ok=1, n=0 and codes 61, 61, 20.
