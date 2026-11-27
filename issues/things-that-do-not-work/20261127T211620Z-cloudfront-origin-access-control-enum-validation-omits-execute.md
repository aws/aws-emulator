Issue-Source: trace-sweep-divergence
Issue-Trace: cloudfront-cfoac-oac-update-enum-casing-mistakes
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudfront-cfoac-oac-update-enum-casing-mistakes
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront|UpdateOriginAccessControl|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Commit: 9035f96a24b7bc6854e584ea99175d352b190dfa

# CloudFront origin-access-control enum validation omits execute-api from its accepted value set

> _Note: this issue was identified against the code base at commit `9035f96a24b7bc6854e584ea99175d352b190dfa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateOriginAccessControl

## Description
Cause 1 of 1: UpdateOriginAccessControl's enum-constraint refusal reports a smaller accepted value set than AWS's, omitting execute-api.
AWS: Smithy: OriginAccessControlConfig.OriginAccessControlOriginType ("The type of origin that this origin access control is for") targets the enum OriginAccessControlOriginTypes. AWS refuses a wrongly cased or unknown value with 400 and the standard constraint sentence: "1 validation error detected: Value '<submitted value>' at '<member path>' failed to satisfy constraint: Member must satisfy enum value set: [s3, execute-api, lambda, mediastore, mediapackagev2]". The fixed text is the whole sentence and the bracketed set; the variable parts are the submitted value ('S3' in the recording) and the member path, both taken from the request. The accepted set is a property of the service: AWS supports API Gateway (execute-api) origins for origin access control.
Emulator: returns the same 400 with the set rendered as '[s3, lambda, mediastore, mediapackagev2]' — execute-api is missing. The on-disk model in this checkout is the source of that set: OriginAccessControlOriginTypes enumerates only [s3, mediastore, mediapackagev2, lambda].
Why: the message is the only place a client (or a human reading an SDK error) learns which origin types the API accepts, so a caller is told execute-api is not a legal value. The same stale set is what the validator checks against, so a well-formed CreateOriginAccessControl/UpdateOriginAccessControl naming 'execute-api' — the supported way to protect an API Gateway origin — is refused outright rather than accepted.
Fix: the OriginAccessControl validation's accepted enum set (and the OriginAccessControlOriginTypes values the model feeds it) — accept execute-api and render the set in AWS's order when composing the constraint message.
Traces: cloudfront-cfoac-oac-update-enum-casing-mistakes#task#4.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (score 94.6%, 32 divergence clusters, all cloudfront).

cloudfront-cfoac-oac-update-enum-casing-mistakes#task#4 UpdateOriginAccessControl (400/400, only the message diverges):
expected $.ErrorResponse.Error.Message = "1 validation error detected: Value 'S3' at 'originAccessControlConfig.originAccessControlOriginType' failed to satisfy constraint: Member must satisfy enum value set: [s3, execute-api, lambda, mediastore, mediapackagev2]"
actual   $.ErrorResponse.Error.Message = "1 validation error detected: Value 'S3' at 'originAccessControlConfig.originAccessControlOriginType' failed to satisfy constraint: Member must satisfy enum value set: [s3, lambda, mediastore, mediapackagev2]"
Model in checkout: services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/smithy-model — OriginAccessControlOriginTypes = [s3, mediastore, mediapackagev2, lambda] (no execute-api).

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the simparity-wire-aws-v1 wire corpus.
3. Replay fixture cloudfront-cfoac-oac-update-enum-casing-mistakes and inspect task#4: PUT an OriginAccessControlConfig whose OriginAccessControlOriginType is 'S3' (wrong case) and compare the 400 ErrorResponse message's bracketed enum set against AWS's [s3, execute-api, lambda, mediastore, mediapackagev2].
4. Also confirm that a valid 'execute-api' OriginAccessControlOriginType is accepted rather than refused by the same validator.
