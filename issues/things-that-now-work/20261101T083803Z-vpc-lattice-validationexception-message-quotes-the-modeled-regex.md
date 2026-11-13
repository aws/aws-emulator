Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: vpc-lattice.resource-policy-survives-unrelated-update-then-clears
Issue-Shape-Id: com.amazonaws.vpclattice#MercuryControlPlane
Issue-Key: vpc-lattice.resource-policy-survives-unrelated-update-then-clears|run#22|AssertionError|assert error == ("ValidationException", expected_message), (
Issue-Commit: f3acd2b65691ed252d07fa7cacd8ea172487fbec

# vpc-lattice ValidationException message quotes the modeled regex with its ^ and $ anchors, diverging from AWS's pattern text

> _Note: this issue was identified against the code base at commit `f3acd2b65691ed252d07fa7cacd8ea172487fbec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResourcePolicy

## Description
Cause 1 of 1: the shared constraint-violation message builder renders a `@pattern` constraint failure by splicing the Smithy trait's pattern string verbatim, including the leading `^` and trailing `$` anchors. AWS renders the same message with the anchors stripped, so every pattern-constrained member of this service reports a message that no client-side or test-side comparison against real AWS can match. Failing check: vpc-lattice.resource-policy-survives-unrelated-update-then-clears.

AWS: `GetResourcePolicy.resourceArn` is a required `@httpLabel` targeting `ResourceArn`, modeled `@length(min: 20, max: 200)` and `@pattern("^arn(:[a-z0-9]+([.-][a-z0-9]+)*){2}(:([a-z0-9]+([.-][a-z0-9]+)*)?){2}:((servicenetwork/sn)|(service/svc)|(resourceconfiguration/rcfg))-[0-9a-z]{17}$")`; the modeled error for a constraint failure is `ValidationException` (HTTP 400) with required members `message` and `reason` (`ValidationExceptionReason` values unknownOperation|cannotParse|fieldValidationFailed|other) and optional `fieldList` (Smithy: services/service-com-amazonaws-vpclattice-mercurycontrolplane/smithy-model/com-amazonaws-vpclattice-mercurycontrolplane.json, GetResourcePolicy closure). The model does not specify the message TEXT; recorded behavior is that AWS answers HTTP 400 / `ValidationException` with the fixed text `1 validation error detected: Value '<value>' at '<member>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>`, where `<value>` is the rejected value from the request, `<member>` is the input member's wire name, and `<pattern>` is that member's modeled regular expression WITHOUT the surrounding `^`/`$` anchors — AWS quotes the bare expression, e.g. `arn(:[a-z0-9]+([.-][a-z0-9]+)*){2}(:([a-z0-9]+([.-][a-z0-9]+)*)?){2}:((servicenetwork/sn)|(service/svc)|(resourceconfiguration/rcfg))-[0-9a-z]{17}`. Hypothesis: AWS's validator stores the expression unanchored and anchors it at match time, so only the unanchored form ever reaches the message.

Emulator: the status (400) and the error code (`ValidationException`) are correct, and all the fixed text matches, but the emulator emits the pattern WITH anchors: `... Member must satisfy regular expression pattern: ^arn(:[a-z0-9]+([.-][a-z0-9]+)*){2}(:([a-z0-9]+([.-][a-z0-9]+)*)?){2}:((servicenetwork/sn)|(service/svc)|(resourceconfiguration/rcfg))-[0-9a-z]{17}$`. The only divergence is the extra leading `^` and trailing `$`. Because the pattern text is taken straight from the model trait, this affects every pattern-constrained member of the service, not just `resourceArn`.

Why: a client (or a parity test, or any code that parses the validation message to surface the offending constraint to a user) that compares the message AWS words for this error class sees a mismatch on every single pattern rejection in the service. Callers that extract the regex from the message and re-apply it client-side get an expression that is not the one AWS published, and the emulator is unusable for asserting on refusal messages.

Fix: the shared constraint-validation message builder reached from the centralized request walk's validate step — the pattern-constraint rendering in `aws_smithy_schema_server::constraints` (the `Violation`/`ViolationKind`/`envelope` path that `services/service-com-amazonaws-vpclattice-mercurycontrolplane/src/state.rs` imports at line 15 to construct `ValidationException`/`ValidationExceptionField`). It must strip a leading `^` and a trailing `$` from the modeled pattern before splicing it into the message, while still anchoring the match itself. Not a change to `ops/get_resource_policy.rs`, which never reaches its handler for this request.

Note: no open vpc-lattice issue covers this — the nearest ones (20261030T043524Z-vpc-lattice-minted-resource-ids-use-a-base36.md and 20261030T175927Z-vpc-lattice-mints-rule-and-target-group-ids.md) concern the id alphabet used when MINTING ids, not the rendering of a constraint-violation message.

Traces: vpc-lattice.resource-policy-survives-unrelated-update-then-clears (final assertion, after the whole put/survive/delete round trip has already passed).

## Evidence
Check vpc-lattice.resource-policy-survives-unrelated-update-then-clears, [mismatch]:

  AssertionError: reading a resource policy by the bare id 'sn-a2houcp5iea9srcao' (not a full ARN) raised
  ('ValidationException', "1 validation error detected: Value 'sn-a2houcp5iea9srcao' at 'resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn(:[a-z0-9]+([.-][a-z0-9]+)*){2}(:([a-z0-9]+([.-][a-z0-9]+)*)?){2}:((servicenetwork/sn)|(service/svc)|(resourceconfiguration/rcfg))-[0-9a-z]{17}$"),
  expected
  ('ValidationException', "1 validation error detected: Value 'sn-a2houcp5iea9srcao' at 'resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn(:[a-z0-9]+([.-][a-z0-9]+)*){2}(:([a-z0-9]+([.-][a-z0-9]+)*)?){2}:((servicenetwork/sn)|(service/svc)|(resourceconfiguration/rcfg))-[0-9a-z]{17}")

Expected (AWS): HTTP 400, code ValidationException, message
  "1 validation error detected: Value '<value>' at '<member>' failed to satisfy constraint: Member must satisfy regular expression pattern: <unanchored modeled pattern>"
Actual (emulator): HTTP 400, code ValidationException, message identical EXCEPT the pattern carries a leading '^' and a trailing '$'.

Fixed text of the message (requirement): "1 validation error detected: Value '", "' at '", "' failed to satisfy constraint: Member must satisfy regular expression pattern: ".
Variable parts (from the request / the model, not from any one run): <value> = the rejected input value ('sn-a2houcp5iea9srcao' in this run, a per-run minted id — not part of the requirement); <member> = 'resourceArn'; <pattern> = ResourceArn's modeled @pattern with ^ and $ removed.

Model evidence: ResourceArn @pattern is stored anchored in the Smithy model ("^arn(...)...{17}$"), which is why the emulator's splice includes the anchors; AWS's message does not.

## Steps to Reproduce
1. Boot the emulator and point a botocore vpc-lattice client at it.
2. net = create_service_network(name=<unique>, authType="NONE")
3. Call get_resource_policy(resourceArn=net["id"]) — i.e. pass the BARE id (e.g. "sn-...") where a full ARN is required, so the modeled @pattern on resourceArn fails and the request is refused in the validate step before the handler runs.
4. Inspect the ClientError: exc.response["Error"]["Code"] == "ValidationException" and HTTP 400 are correct, but exc.response["Error"]["Message"] ends with "...-[0-9a-z]{17}$" and the quoted pattern begins with '^'.
5. AWS returns the same message with the pattern unanchored: it begins "arn(:..." and ends "...-[0-9a-z]{17}". Any member of this service with a @pattern constraint reproduces the same divergence.
