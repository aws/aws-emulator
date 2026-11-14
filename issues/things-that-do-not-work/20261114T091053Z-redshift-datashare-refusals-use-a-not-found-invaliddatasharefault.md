Issue-Source: trace-sweep-divergence
Issue-Trace: redshift-datashare-authorization-lifecycle-errors
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: redshift-datashare-authorization-lifecycle-errors redshift-authorize-datashare-unknown-producer redshift-deauthorize-datashare-never-authorized redshift-data-share-and-namespace-preconditions
Issue-Shape-Id: com.amazonaws.redshift#RedshiftServiceVersion20121201
Issue-Key: redshift|AuthorizeDataShare|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: redshift|AuthorizeDataShare|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: redshift|DeauthorizeDataShare|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: redshift|DeauthorizeDataShare|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: redshift|DescribeDataShares|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: redshift|RejectDataShare|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: redshift|AssociateDataShareConsumer|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: redshift|DisassociateDataShareConsumer|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Commit: 029c732db8092362c2d6a7aa89bcc4c3ae279d73

# Redshift datashare refusals use a not-found InvalidDataShareFault instead of AWS's per-operation datashare wording and consumer-identifier validation

> _Note: this issue was identified against the code base at commit `029c732db8092362c2d6a7aa89bcc4c3ae279d73`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AuthorizeDataShare

## Description
Every datashare operation in the emulator answers one single refusal: an `InvalidDataShareFault` whose message is the emulator's own template `Datashare <arn> not found.`. AWS distinguishes two different refusals here — an invalid *consumer identifier* (a request-validation refusal, code `InvalidParameterValue`) and a datashare that does not exist (code `InvalidDataShareFault`, with wording that differs between the describe path and the association/authorization path) — and never uses the emulator's wording for either. This issue bundles 8 causes; all 8 must be closed.

Cause 1 of 8: AuthorizeDataShare with a consumer identifier that is not a valid account id/ARN.
AWS: Smithy: AuthorizeDataShare input members DataShareArn (required) and ConsumerIdentifier (required); modeled error InvalidDataShareFault (httpError 400, awsQueryError code InvalidDataShareFault). The recorded response shows AWS validating ConsumerIdentifier BEFORE resolving the datashare and refusing with HTTP 400, `Error.Type` `Sender`, `Error.Code` `InvalidParameterValue` (a query-protocol common sender code, not a modeled operation error — recorded behavior only) and the fixed message text `The consumer identifier isn't valid.` (no variable parts).
Emulator: HTTP 400 with `Error.Code` `InvalidDataShareFault` and message `Datashare arn:aws:redshift:<region>:<account>:datashare:<uuid>/<name> not found.` — i.e. it resolves the datashare first and reports it missing, and never performs the consumer-identifier syntax check.
Why: a client that branches on the error code (retry/repair the consumer id vs. create the datashare) takes the wrong branch, and a client asserting on AWS's message sees an unrelated not-found sentence. Error-code-driven SDK error classes (`InvalidParameterValue` vs `InvalidDataShareFault`) are different exception types.
Fix: AuthorizeDataShare's request validation in the redshift handler — order consumer-identifier validation ahead of datashare lookup and emit the `InvalidParameterValue` sender error with AWS's fixed text.
Traces: redshift-datashare-authorization-lifecycle-errors#task#1.

Cause 2 of 8: DeauthorizeDataShare with an invalid consumer identifier.
AWS: Smithy: DeauthorizeDataShare(DataShareArn required, ConsumerIdentifier required); modeled error InvalidDataShareFault (400). Recorded: HTTP 400, code `InvalidParameterValue`, fixed message `The consumer identifier isn't valid.`
Emulator: HTTP 400, code `InvalidDataShareFault`, message `Datashare <datashare ARN> not found.`
Why: same mis-classification as cause 1 on the deauthorization path, so revoke flows cannot tell a malformed consumer id from a missing share.
Fix: DeauthorizeDataShare's request validation in the redshift handler.
Traces: redshift-datashare-authorization-lifecycle-errors#task#2.

Cause 3 of 8: AuthorizeDataShare against a datashare ARN that does not exist (producer unknown).
AWS: Smithy: modeled error InvalidDataShareFault (httpError 400). Recorded message is the fixed sentence `The data share isn't available because it doesn't exist.` — entirely fixed text, with NO request value interpolated (AWS does not echo the ARN here).
Emulator: HTTP 400 and the right code, but the message is the emulator's template `Datashare <arn> not found.`, which interpolates the request's ARN.
Why: a client matching AWS's documented sentence (or a snapshot/golden test of it) never matches; the emulator also leaks a resource identifier AWS deliberately omits.
Fix: the datashare not-found error construction in the redshift handler's datashare state/error layer — one shared AWS sentence, no ARN interpolation.
Traces: redshift-authorize-datashare-unknown-producer#task#2.

Cause 4 of 8: DeauthorizeDataShare against a datashare that was never authorized/does not exist.
AWS: Smithy: modeled error InvalidDataShareFault (400); recorded fixed message `The data share isn't available because it doesn't exist.`
Emulator: `Datashare <arn> not found.`
Why: as cause 3, on the deauthorize path.
Fix: same datashare not-found error construction.
Traces: redshift-deauthorize-datashare-never-authorized#task#1 (cluster also covers #task#2 of the same fixture).

Cause 5 of 8: DescribeDataShares with a DataShareArn that does not exist.
AWS: Smithy: DescribeDataShares(DataShareArn optional); modeled error InvalidDataShareFault (httpError 400). The describe path has its OWN fixed wording, recorded as `The datashare Amazon Resource Name (ARN) isn't valid because the ARN doesn't exist. Use a valid datashare ARN.` — all fixed text, no request values.
Emulator: HTTP 400 with `Datashare <arn> not found.`
Why: the read path and the mutation paths are distinguishable on AWS by their message; a client (or operator log scraper) that keys off the describe wording sees the mutation-style sentence instead.
Fix: DescribeDataShares' error path in the redshift handler.
Traces: redshift-data-share-and-namespace-preconditions#task#1.

Cause 6 of 8: RejectDataShare against a nonexistent datashare.
AWS: Smithy: RejectDataShare(DataShareArn required); modeled error InvalidDataShareFault (400); recorded fixed message `The data share isn't available because it doesn't exist.`
Emulator: `Datashare <arn> not found.`
Fix: RejectDataShare's error path / shared datashare not-found error.
Why: as cause 3.
Traces: redshift-data-share-and-namespace-preconditions#task#2.

Cause 7 of 8: AssociateDataShareConsumer against a nonexistent datashare.
AWS: Smithy: AssociateDataShareConsumer errors InvalidDataShareFault (400) and InvalidNamespaceFault (400); recorded fixed message `The data share isn't available because it doesn't exist.`
Emulator: `Datashare <arn> not found.`
Why: as cause 3, on the consumer-association path.
Fix: AssociateDataShareConsumer's error path / shared datashare not-found error.
Traces: redshift-data-share-and-namespace-preconditions#task#3.

Cause 8 of 8: DisassociateDataShareConsumer against a nonexistent datashare.
AWS: Smithy: DisassociateDataShareConsumer errors InvalidDataShareFault (400), InvalidNamespaceFault (400); recorded fixed message `The data share isn't available because it doesn't exist.`
Emulator: `Datashare <arn> not found.`
Why: as cause 3, on the disassociation path.
Fix: DisassociateDataShareConsumer's error path / shared datashare not-found error.
Traces: redshift-data-share-and-namespace-preconditions#task#4.

Related but NOT covering these: open issues 20261104T045922Z-redshift-resource-policy-and-namespace-registration-semantics-diverge.md and 20261029T200555Z-redshift-namespace-reserved-node-idc-application-hsm-refusals.md touch namespace/resource-policy refusals; neither carries the replay keys above, and the replay reports these 8 clusters as unowned.

## Evidence
Replay of trace set simparity-wire-aws-v1 (50 fixtures, 307 steps, 70 divergence clusters), all 8 clusters reported with empty owned_by and empty stub_path:

1. redshift-datashare-authorization-lifecycle-errors#task#1 AuthorizeDataShare — status 400 vs 400; $.ErrorResponse.Error.Code: expected 'InvalidParameterValue', got 'InvalidDataShareFault'; $.ErrorResponse.Error.Message: expected "The consumer identifier isn't valid.", got 'Datashare arn:aws:redshift:us-west-2:000000000000:datashare:d0286757-d2eb-4a21-a2c0-e0d0b063e97d/sim2real-probe-datashare not found.'
2. redshift-datashare-authorization-lifecycle-errors#task#2 DeauthorizeDataShare — Code expected 'InvalidParameterValue', got 'InvalidDataShareFault'; Message expected "The consumer identifier isn't valid.", got 'Datashare arn:...:datashare:ac8d62a0-.../sim2real-probe-datashare2 not found.'
3. redshift-authorize-datashare-unknown-producer#task#2 AuthorizeDataShare — Message expected "The data share isn't available because it doesn't exist.", got 'Datashare arn:...:datashare:4e3b04ff-.../sim2real-probe-redshift-datashare-5c742dca not found.'
4. redshift-deauthorize-datashare-never-authorized#task#1 DeauthorizeDataShare — Message expected "The data share isn't available because it doesn't exist.", got 'Datashare arn:...:datashare:2d32d2b3-.../sim2real-probe-redshift-datashare-revoke-2e1b3203 not found.' (affected_count 2: also #task#2)
5. redshift-data-share-and-namespace-preconditions#task#1 DescribeDataShares — Message expected "The datashare Amazon Resource Name (ARN) isn't valid because the ARN doesn't exist. Use a valid datashare ARN.", got 'Datashare arn:...:datashare:11111111-1111-1111-1111-111111111111/sim2real_probe_share not found.'
6. redshift-data-share-and-namespace-preconditions#task#2 RejectDataShare — Message expected "The data share isn't available because it doesn't exist.", got 'Datashare arn:...11111111-.../sim2real_probe_share not found.'
7. redshift-data-share-and-namespace-preconditions#task#3 AssociateDataShareConsumer — same expected vs actual as (6).
8. redshift-data-share-and-namespace-preconditions#task#4 DisassociateDataShareConsumer — same expected vs actual as (6).

Fixed vs variable text: AWS's sentences here are entirely fixed (`The consumer identifier isn't valid.`, `The data share isn't available because it doesn't exist.`, `The datashare Amazon Resource Name (ARN) isn't valid because the ARN doesn't exist. Use a valid datashare ARN.`). The ARNs/UUIDs/names shown above are per-run values from the request and are NOT part of any required message.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus) for trace set simparity-wire-aws-v1.
2. Cause 1/2: call AuthorizeDataShare / DeauthorizeDataShare with any DataShareArn and a ConsumerIdentifier that is not a valid account id or namespace ARN (fixture redshift-datashare-authorization-lifecycle-errors, steps task#1 and task#2). Observe HTTP 400 with Error.Code InvalidDataShareFault and a 'Datashare ... not found.' message; AWS answers Error.Code InvalidParameterValue with "The consumer identifier isn't valid.".
3. Causes 3/4/6/7/8: call AuthorizeDataShare, DeauthorizeDataShare, RejectDataShare, AssociateDataShareConsumer and DisassociateDataShareConsumer with a datashare ARN that no datashare in state matches (fixtures redshift-authorize-datashare-unknown-producer#task#2, redshift-deauthorize-datashare-never-authorized#task#1, redshift-data-share-and-namespace-preconditions#task#2/#task#3/#task#4). Observe the emulator's 'Datashare <arn> not found.' message instead of AWS's "The data share isn't available because it doesn't exist.".
4. Cause 5: call DescribeDataShares with DataShareArn set to a nonexistent datashare (redshift-data-share-and-namespace-preconditions#task#1). Observe the same emulator template instead of AWS's describe-path sentence.
5. Re-replay the listed fixture steps after the fix; all 8 must pass.
