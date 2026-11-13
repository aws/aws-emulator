Issue-Source: trace-sweep-divergence
Issue-Trace: mq-configuration-lifecycle-and-revisions
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mq-configuration-lifecycle-and-revisions
Issue-Shape-Id: com.amazonaws.mq#mq
Issue-Key: mq|DescribeConfigurationRevision|2xx->2xx|$.data
Issue-Key: mq|ListConfigurationRevisions|2xx->2xx|$.nextToken
Issue-Commit: 63c612bdac710916216a1ca8dd5dbf0b76505045

# mq configuration revision reads diverge from recorded Amazon MQ responses

> _Note: this issue was identified against the code base at commit `63c612bdac710916216a1ca8dd5dbf0b76505045`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeConfigurationRevision

## Description
Two causes in the mq configuration/revision read path, both on fixture mq-configuration-lifecycle-and-revisions. Both steps answer HTTP 200 with the right status and shape; the response BODY diverges. Fix both before closing.

Cause 1 of 2: DescribeConfigurationRevision returns the submitted configuration document byte-for-byte instead of the canonicalized document Amazon MQ stores for the revision (fixture mq-configuration-lifecycle-and-revisions, step task#4).
AWS: recorded behavior only for the canonicalization rule — Smithy (services/service-com-amazonaws-mq-mq/smithy-model/com-amazonaws-mq-mq.json) models DescribeConfigurationRevision as GET /v1/configurations/{ConfigurationId}/revisions/{ConfigurationRevision}, 200, with output member Data (jsonName "data", __string) documented as "Amazon MQ for ActiveMQ: the base64-encoded XML configuration"; it states nothing about normalization. The recording shows that Amazon MQ does NOT echo the bytes a CreateConfiguration/UpdateConfiguration request sent: it parses the ActiveMQ XML document, stores a canonical serialization of it, and returns the base64 of THAT. In the recorded step the stored revision document comes back with the empty element written in self-closed form (`<plugins/>`), whereas the document the fixture submitted spells the same empty element as an open/close pair on separate lines (`<plugins>` newline+indent `</plugins>`). UpdateConfiguration's modeled Warnings member (__listOfSanitizationWarning, "warnings about the configuration elements or attributes that were sanitized") is the model's own evidence that the service rewrites the submitted document rather than storing it opaquely.
Emulator: UpdateConfiguration validates only that Data is non-empty and base64-decodable and then stores the request string unchanged (services/service-com-amazonaws-mq-mq/src/ops/update_configuration.rs, the ConfigurationRevisionRecord::accepted(..., Some(data)) call), and DescribeConfigurationRevision hands that same string back (describe_configuration_revision.rs, `data: revision.data.clone()`). So the replay got base64 of the document with `<plugins>\n  </plugins>` where the recording has base64 of the document with `<plugins/>`; every other member of the 200 matched.
Why: a client that writes a configuration and reads the revision back gets a document that is not what AWS would have stored. Anything that compares, diffs, hashes or digests the returned `data` against the canonical form AWS serves (drift detection, config-as-code reconciliation, or a checksum of the revision body) reports a spurious difference against real AWS and a spurious match against the emulator, and a client relying on the service to normalize its input never sees the normalization.
Fix: belongs in the configuration-revision state/write path — canonicalize the decoded document when a revision is recorded (update_configuration.rs / create_configuration.rs via state's revision record) so every reader, including describe_configuration_revision.rs's `data` and DescribeConfiguration, reports the canonical serialization; only an empty element's spelling is at issue here, not the document's content.

Cause 2 of 2: ListConfigurationRevisions omits nextToken on the last page, where Amazon MQ always returns one (fixture mq-configuration-lifecycle-and-revisions, step task#5).
AWS: recorded behavior only — Smithy models ListConfigurationRevisions as GET /v1/configurations/{ConfigurationId}/revisions, 200, output members ConfigurationId, MaxResults, NextToken (jsonName "nextToken", __string) and Revisions; the model marks NextToken optional and does not say when it is emitted. The recorded response for this configuration carries a nextToken STRING even though the whole revision chain fits in one page (the fixture asserts the member is present and shaped, not its literal value — the token value itself is a per-run opaque value and is not a requirement).
Emulator: list_configuration_revisions.rs sets `next_token: page.next_token`, which state::page_of only populates when a further page exists, so the member is absent/null on the last page. Replay: `$.nextToken: missing (expected '<shaped-not-asserted>', got None)`; status 200 and all other members matched.
Why: a client that paginates by presence of nextToken behaves differently against the emulator than against AWS, and a client that asserts the field exists (or that round-trips the token into a follow-up call) fails only on the emulator — the emulator's listing surface looks like a terminated pagination where AWS's does not.
Fix: the ListConfigurationRevisions handler's pagination/serialization of the page token (services/service-com-amazonaws-mq-mq/src/ops/list_configuration_revisions.rs together with state's page_of helper) — emit a nextToken on every page of this listing, including the last, and keep it a token the handler's own state::decode_page_token accepts.

Neither cause is covered by an open issue: the replay marked these two clusters with no owner, while the sibling mq clusters in the same run (DescribeBrokerInstanceOptions, CreateTags/DeleteTags content-length) are owned by 20261030T033328Z-mq-describebrokerinstanceoptions-serves-a-hand-built-instance-option.md and 20261030T154619Z-mq-tagging-operations-omit-content-length-from-their.md respectively and are excluded here.

## Evidence
Fixture mq-configuration-lifecycle-and-revisions, step task#4 (DescribeConfigurationRevision), expected 200 / actual 200:
  $.data: value_mismatch
  expected (base64) decodes to:
    <?xml version="1.0" encoding="UTF-8" standalone="yes"?>
    <broker xmlns="http://activemq.apache.org/schema/core">
      <plugins/>
    </broker>
  actual (base64) decodes to:
    <?xml version="1.0" encoding="UTF-8" standalone="yes"?>
    <broker xmlns="http://activemq.apache.org/schema/core">
      <plugins>
      </plugins>
    </broker>
  i.e. the emulator returns the submitted bytes; AWS returns its canonical serialization (empty element self-closed). The XML prolog, the broker element and its xmlns are fixed text of the document under test; the element set is whatever the request supplied.

Fixture mq-configuration-lifecycle-and-revisions, step task#5 (ListConfigurationRevisions), expected 200 / actual 200:
  $.nextToken: missing (expected '<shaped-not-asserted>', got None)
  The recorded AWS response carries nextToken as a string on this single-page listing; the emulator returns the member absent/null.

Replay summary: trace_set simparity-wire-aws-v1, emulator_up true, 26 steps, 20 passed, 6 mismatched, score 76.9%.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it; replay the wire corpus (trace set simparity-wire-aws-v1).
2. Cause 1 — replay fixture mq-configuration-lifecycle-and-revisions through step task#4: create an ActiveMQ configuration, UpdateConfiguration it with base64 of an ActiveMQ XML document containing an empty element written as an open/close pair (`<plugins>\n  </plugins>`), then GET /v1/configurations/{ConfigurationId}/revisions/{revision}. Base64-decode the response's `data`: the emulator returns the exact submitted document, AWS returns the canonicalized one (`<plugins/>`).
3. Cause 2 — same fixture, step task#5: GET /v1/configurations/{ConfigurationId}/revisions with the whole revision chain fitting in one page. The emulator's 200 body has no `nextToken` member; the recorded AWS 200 body carries a nextToken string.
