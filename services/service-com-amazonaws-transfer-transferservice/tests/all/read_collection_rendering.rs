// SPDX-License-Identifier: CC0-1.0

//! How a READ renders the collections a resource holds, and how it tags the elements of
//! a workflow's step lists.
//!
//! RULE 1: a member reporting a collection the resource ALWAYS holds — its tag set, a
//! profile's attached certificate ids, a workflow's two step lists — is always
//! serialised, as the EMPTY list when that collection is empty, because "the resource
//! holds none" and "the member was not reported" are different answers and a caller
//! that reads the member unconditionally must get the first one. A member reporting an
//! optional setting the resource does NOT have (a workflow's log destination) stays
//! absent, which is what makes those two answers distinguishable at all.
//!
//! RULE 2: each element of a `WorkflowSteps` list is rendered with a type discriminator
//! naming the element's own shape, alongside the step's modelled members — a list
//! element only, so neither the enclosing workflow nor the step's own `*StepDetails`
//! structure carries one.
//!
//! The rows differ along what the rules generalise over: the resource KIND the read
//! addresses (every kind this service stores a tag set for, plus the two readers whose
//! other collection members this covers), the SIZE of the collection (none, one, three,
//! and a workflow's whole modelled `len 0..8` step range), the step TYPE each element
//! carries, and which of the two step lists the element sits in.
//!
//! Every request goes over the real wire path (`POST /` with
//! `X-Amz-Target: TransferService.<Op>`) through the shared [`crate::harness`], so what
//! is asserted is what an SDK sees rather than what a handler returns.

use serde_json::{json, Value};

use crate::harness::{generated_host_key, member_of, Emulator, ROLE};

/// An IAM Identity Center instance ARN satisfying `IdentityCenterInstanceArn`'s
/// modelled pattern `^arn:[\w-]+:sso:::instance/(sso)?ins-[a-zA-Z0-9-.]{16}$`.
const INSTANCE_ARN: &str = "arn:aws:sso:::instance/ssoins-1234567890abcdef";

/// The member name the discriminator of a step element travels under, and the value it
/// carries: the local name of the `com.amazonaws.transfer#WorkflowStep` shape whose
/// elements it tags.
const STEP_DISCRIMINATOR: (&str, &str) = ("__type", "WorkflowStep");

/// Every `WorkflowStepType` value, in the order the model declares it, with the ONE
/// `WorkflowStep` member that configures it. Source: `smithy_query(query="shape",
/// shape="com.amazonaws.transfer#WorkflowStep")`.
const STEP_TYPES: [(&str, &str); 5] = [
    ("COPY", "CopyStepDetails"),
    ("CUSTOM", "CustomStepDetails"),
    ("TAG", "TagStepDetails"),
    ("DELETE", "DeleteStepDetails"),
    ("DECRYPT", "DecryptStepDetails"),
];

/// `Steps` and `OnExceptionSteps` are modelled `len 0..8`
/// (`smithy_query(query="shape",shape="com.amazonaws.transfer#WorkflowSteps")`), so a
/// workflow carries at most this many steps per list.
const MAX_STEPS: usize = 8;

/// Satisfies `CustomStepTarget`'s modelled pattern `^arn:[a-z-]+:lambda:.*$`.
const LAMBDA: &str = "arn:aws:lambda:us-east-1:000000000000:function:on-upload";

/// One resource read, created over the wire: how it is addressed, which member of the
/// answer carries the resource, and which of that resource's members report a
/// collection it always holds.
struct Read {
    /// What this row varies, named in every assertion it makes.
    case: String,
    describe_op: &'static str,
    describe_request: Value,
    describe_member: &'static str,
    /// The ARN the tag plane addresses this resource by.
    arn: String,
    /// The members whose value must be reported as a list whatever the resource holds.
    always_reported: &'static [&'static str],
}

/// A `Tags` request member from `pairs`.
fn tag_list(pairs: &[(&str, &str)]) -> Value {
    Value::Array(
        pairs
            .iter()
            .map(|(key, value)| json!({"Key": key, "Value": value}))
            .collect(),
    )
}

/// A step of `step_type` named `name`, satisfying every member the step requires.
fn step_of(step_type: &str, name: &str) -> Value {
    let details_member = STEP_TYPES
        .iter()
        .find(|(known, _)| *known == step_type)
        .map(|(_, member)| *member)
        .unwrap_or_else(|| panic!("{step_type} is not a modelled WorkflowStepType"));
    let details = match step_type {
        "COPY" => json!({
            "Name": name,
            "DestinationFileLocation": {"S3FileLocation": {"Bucket": "step-target", "Key": "out/"}},
        }),
        "CUSTOM" => json!({"Name": name, "Target": LAMBDA, "TimeoutSeconds": 60}),
        "TAG" => json!({"Name": name, "Tags": [{"Key": "scanned", "Value": "yes"}]}),
        "DELETE" => json!({"Name": name}),
        "DECRYPT" => json!({
            "Name": name,
            "Type": "PGP",
            "DestinationFileLocation": {"S3FileLocation": {"Bucket": "step-clear", "Key": "out/"}},
        }),
        other => panic!("{other} is not a modelled WorkflowStepType"),
    };
    json!({"Type": step_type, details_member: details})
}

/// Call `op` and assert it answered 200, handing back the decoded body.
async fn ok(emulator: &Emulator, case: &str, op: &str, request: Value) -> Value {
    let answer = emulator.call(op, request).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected {op} 200, got {} with body {}",
        answer.status, answer.body
    );
    answer.body
}

/// A minted identifier out of a create's answer.
fn minted(case: &str, body: &Value, member: &str) -> String {
    member_of(body, member)
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the create returned no {member}: {body}"))
        .to_string()
}

/// Read one resource back and hand back the structure its reader reports.
async fn describe(emulator: &Emulator, read: &Read) -> Value {
    let body = ok(
        emulator,
        &read.case,
        read.describe_op,
        read.describe_request.clone(),
    )
    .await;
    member_of(&body, read.describe_member).clone()
}

/// Build the read for `kind`, creating the resource over the wire with NO tags and
/// nothing optional, and resolving the ARN its own reader reports for it.
async fn read_of(emulator: &Emulator, kind: &str) -> Read {
    let case = format!("{kind} read");
    let (describe_op, describe_request, describe_member, always_reported): (
        &'static str,
        Value,
        &'static str,
        &'static [&'static str],
    ) = match kind {
        "server" => {
            let server_id = emulator.create_server(json!({})).await;
            (
                "DescribeServer",
                json!({"ServerId": server_id}),
                "Server",
                &["Tags"],
            )
        }
        "user" => {
            let server_id = emulator.create_server(json!({})).await;
            emulator.create_user(&server_id, "collection-user").await;
            (
                "DescribeUser",
                json!({"ServerId": server_id, "UserName": "collection-user"}),
                "User",
                &["Tags"],
            )
        }
        "host key" => {
            let server_id = emulator.create_server(json!({})).await;
            let (material, _fingerprint) = generated_host_key();
            let created = ok(
                emulator,
                &case,
                "ImportHostKey",
                json!({"ServerId": &server_id, "HostKeyBody": material}),
            )
            .await;
            let host_key_id = minted(&case, &created, "HostKeyId");
            (
                "DescribeHostKey",
                json!({"ServerId": server_id, "HostKeyId": host_key_id}),
                "HostKey",
                &["Tags"],
            )
        }
        "web app" => {
            let created = ok(
                emulator,
                &case,
                "CreateWebApp",
                json!({
                    "IdentityProviderDetails": {
                        "IdentityCenterConfig": {"InstanceArn": INSTANCE_ARN, "Role": ROLE},
                    },
                }),
            )
            .await;
            let web_app_id = minted(&case, &created, "WebAppId");
            (
                "DescribeWebApp",
                json!({"WebAppId": web_app_id}),
                "WebApp",
                &["Tags"],
            )
        }
        "AS2 profile" => {
            let profile_id = emulator
                .create_profile(json!({"As2Id": "collection-as2", "ProfileType": "LOCAL"}))
                .await;
            (
                "DescribeProfile",
                json!({"ProfileId": profile_id}),
                "Profile",
                // A profile's attached certificates are a collection it always holds,
                // and are what a caller inspects to decide whether any remain.
                &["Tags", "CertificateIds"],
            )
        }
        "connector" => {
            let local = emulator
                .create_profile(json!({"As2Id": "connector-local", "ProfileType": "LOCAL"}))
                .await;
            let partner = emulator
                .create_profile(json!({"As2Id": "connector-partner", "ProfileType": "PARTNER"}))
                .await;
            let created = ok(
                emulator,
                &case,
                "CreateConnector",
                json!({
                    "AccessRole": ROLE,
                    "Url": "https://partner.example.com/as2",
                    "As2Config": {
                        "LocalProfileId": local,
                        "PartnerProfileId": partner,
                        "Compression": "DISABLED",
                        "EncryptionAlgorithm": "NONE",
                        "SigningAlgorithm": "NONE",
                    },
                }),
            )
            .await;
            let connector_id = minted(&case, &created, "ConnectorId");
            (
                "DescribeConnector",
                json!({"ConnectorId": connector_id}),
                "Connector",
                &["Tags"],
            )
        }
        "AS2 agreement" => {
            let server_id = emulator.create_server(json!({})).await;
            let local = emulator
                .create_profile(json!({"As2Id": "agreement-local", "ProfileType": "LOCAL"}))
                .await;
            let partner = emulator
                .create_profile(json!({"As2Id": "agreement-partner", "ProfileType": "PARTNER"}))
                .await;
            let created = ok(
                emulator,
                &case,
                "CreateAgreement",
                json!({
                    "ServerId": &server_id,
                    "LocalProfileId": local,
                    "PartnerProfileId": partner,
                    "AccessRole": ROLE,
                    "BaseDirectory": "/as2-inbound",
                }),
            )
            .await;
            let agreement_id = minted(&case, &created, "AgreementId");
            (
                "DescribeAgreement",
                json!({"ServerId": server_id, "AgreementId": agreement_id}),
                "Agreement",
                &["Tags"],
            )
        }
        "certificate" => {
            let created = ok(
                emulator,
                &case,
                "ImportCertificate",
                json!({"Usage": "SIGNING", "Certificate": self_signed_certificate()}),
            )
            .await;
            let certificate_id = minted(&case, &created, "CertificateId");
            (
                "DescribeCertificate",
                json!({"CertificateId": certificate_id}),
                "Certificate",
                &["Tags"],
            )
        }
        "workflow" => {
            let created = ok(
                emulator,
                &case,
                "CreateWorkflow",
                json!({"Steps": [step_of("DELETE", "delete-step")]}),
            )
            .await;
            let workflow_id = minted(&case, &created, "WorkflowId");
            (
                "DescribeWorkflow",
                json!({"WorkflowId": workflow_id}),
                "Workflow",
                // Both step lists are collections the workflow always holds, so the
                // list it was created without is reported empty.
                &["Tags", "Steps", "OnExceptionSteps"],
            )
        }
        other => panic!("{other} is not a resource kind this service stores"),
    };

    let mut read = Read {
        case,
        describe_op,
        describe_request,
        describe_member,
        arn: String::new(),
        always_reported,
    };
    let described = describe(emulator, &read).await;
    read.arn = member_of(&described, "Arn")
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case {:?}: the created resource reports no Arn: {described}",
                read.case
            )
        })
        .to_string();
    read
}

/// A real self-signed certificate in PEM, built by `rcgen` rather than by the code under
/// test, for the one row that imports a certificate.
fn self_signed_certificate() -> String {
    let key = rcgen::KeyPair::generate().expect("a key pair");
    let mut params =
        rcgen::CertificateParams::new(vec!["as2.example.com".to_string()]).expect("parameters");
    let mut subject = rcgen::DistinguishedName::new();
    subject.push(rcgen::DnType::CommonName, "as2.example.com");
    params.distinguished_name = subject;
    params.not_before = rcgen::date_time_ymd(2020, 1, 1);
    params.not_after = rcgen::date_time_ymd(2100, 1, 1);
    params
        .self_signed(&key)
        .expect("a self-signed certificate")
        .pem()
}

/// Every resource kind this suite reads, one row each.
const KINDS: [&str; 9] = [
    "server",
    "user",
    "host key",
    "web app",
    "AS2 profile",
    "connector",
    "AS2 agreement",
    "certificate",
    "workflow",
];

/// The tag sets the size dimension runs over: none, one pair, three pairs.
fn tag_sizes() -> Vec<(&'static str, Vec<(&'static str, &'static str)>)> {
    vec![
        ("no tags", Vec::new()),
        ("one tag", vec![("env", "prod")]),
        (
            "three tags",
            vec![("env", "prod"), ("team", "files"), ("cost-center", "42")],
        ),
    ]
}

/// Assert `member` of `described` is a list of exactly `want` entries.
fn assert_list_len(case: &str, described: &Value, member: &str, want: usize) {
    let actual = member_of(described, member);
    let list = actual.as_array().unwrap_or_else(|| {
        panic!("case {case:?}: expected {member} to be reported as a list, got {actual:?}")
    });
    assert_eq!(
        list.len(),
        want,
        "case {case:?}: expected {member} to report {want} entries, got {actual:?}"
    );
}

/// RULE 1, over every reader: a collection the resource always holds is reported as a
/// list whatever its size — including the empty list — and the optional setting a
/// workflow does not have is reported not at all.
#[tokio::test]
async fn a_read_reports_a_collection_the_resource_holds_as_a_list_at_every_size() {
    let emulator = Emulator::new();

    for kind in KINDS {
        let read = read_of(&emulator, kind).await;

        for (size, pairs) in tag_sizes() {
            let case = format!("{kind}, {size}");
            // Replace the whole tag set with this row's: `TagResource` sets the value
            // of every key it sends, so a row's own keys are the resource's set once
            // the previous row's are detached.
            let listed = ok(
                &emulator,
                &case,
                "ListTagsForResource",
                json!({"Arn": &read.arn}),
            )
            .await;
            let present: Vec<String> = member_of(&listed, "Tags")
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|tag| {
                    member_of(tag, "Key")
                        .as_str()
                        .unwrap_or_default()
                        .to_string()
                })
                .collect();
            if !present.is_empty() {
                ok(
                    &emulator,
                    &case,
                    "UntagResource",
                    json!({"Arn": &read.arn, "TagKeys": present}),
                )
                .await;
            }
            if !pairs.is_empty() {
                ok(
                    &emulator,
                    &case,
                    "TagResource",
                    json!({"Arn": &read.arn, "Tags": tag_list(&pairs)}),
                )
                .await;
            }

            // The resource's own reader reports the tag set, and every other
            // collection it always holds, as a list of the size it holds.
            let described = describe(&emulator, &read).await;
            assert_list_len(&case, &described, "Tags", pairs.len());
            for member in read.always_reported {
                let actual = member_of(&described, member);
                assert!(
                    actual.is_array(),
                    "case {case:?}: expected {member} to be reported as a list, got {actual:?}"
                );
            }

            // `ListTagsForResource` is `@paginated` over `Tags`, so the items member is
            // reported on every call — the same set, at the same size.
            let listed = ok(
                &emulator,
                &case,
                "ListTagsForResource",
                json!({"Arn": &read.arn}),
            )
            .await;
            assert_list_len(&case, &listed, "Tags", pairs.len());
        }
    }
}

/// RULE 1, the other side: `ListTagsForResource` reports the items member even for an
/// `Arn` no stored resource answers to — the operation models no not-found error, so
/// such an ARN simply has no tags.
#[tokio::test]
async fn the_tag_listing_reports_an_empty_items_member_for_an_unknown_arn() {
    let emulator = Emulator::new();
    let rows = [
        (
            "a server id nothing minted",
            "arn:aws:transfer:us-east-1:000000000000:server/s-fffffffffffffffff",
        ),
        (
            "a workflow nothing answers to",
            "arn:aws:transfer:us-east-1:000000000000:workflow/w-00000000000000001",
        ),
        ("an ARN of another service entirely", "arn:aws:s3:::a-bucket"),
    ];

    for (case, arn) in rows {
        let listed = ok(&emulator, case, "ListTagsForResource", json!({"Arn": arn})).await;
        assert_list_len(case, &listed, "Tags", 0);
        assert_eq!(
            member_of(&listed, "Arn").as_str(),
            Some(arn),
            "case {case:?}: expected the requested Arn echoed, got {listed}"
        );
    }
}

/// RULE 1 for the member that is NOT such a collection: a workflow's log destination is
/// a setting the workflow either has or does not, so a workflow configured without one
/// reports no member at all — while its empty step list and its empty tag set are still
/// reported.
#[tokio::test]
async fn a_workflow_read_omits_the_log_destination_it_was_not_given() {
    let emulator = Emulator::new();
    // `StructuredLogDestinations`' member shape is a CloudWatch log group ARN.
    let log_group = "arn:aws:logs:us-east-1:000000000000:log-group:transfer-flows:*";
    let rows: Vec<(&str, Value, Value)> = vec![
        (
            "no log destination",
            json!({"Steps": [step_of("DELETE", "delete-step")]}),
            Value::Null,
        ),
        (
            "one log destination",
            json!({
                "Steps": [step_of("DELETE", "delete-step")],
                "StructuredLogDestinations": [log_group],
            }),
            json!([log_group]),
        ),
    ];

    for (case, request, expected) in rows {
        let created = ok(&emulator, case, "CreateWorkflow", request).await;
        let workflow_id = minted(case, &created, "WorkflowId");
        let body = ok(
            &emulator,
            case,
            "DescribeWorkflow",
            json!({"WorkflowId": workflow_id}),
        )
        .await;
        let workflow = member_of(&body, "Workflow");
        let actual = member_of(workflow, "StructuredLogDestinations");
        assert_eq!(
            actual, &expected,
            "case {case:?}: expected StructuredLogDestinations {expected:?}, got {actual:?}"
        );
        assert_list_len(case, workflow, "Tags", 0);
        assert_list_len(case, workflow, "OnExceptionSteps", 0);
    }
}

/// RULE 2: every element of `Steps` and of `OnExceptionSteps` carries the discriminator
/// naming the step shape, and nothing else in the body does.
///
/// The rows run every step-list length the model admits (1..=8), each at a different
/// rotation of the step types, so the discriminator is asserted on an element of every
/// step type and at every position of both lists.
#[tokio::test]
async fn a_workflow_read_tags_every_step_element_with_its_shape_name() {
    let emulator = Emulator::new();
    let (discriminator, shape_name) = STEP_DISCRIMINATOR;

    for length in 1..=MAX_STEPS {
        let case = format!("{length} step(s) per list");
        // A rotation per length, so the type at a given position differs between rows.
        let types: Vec<&str> = (0..length)
            .map(|position| STEP_TYPES[(position + length) % STEP_TYPES.len()].0)
            .collect();
        let steps: Vec<Value> = types
            .iter()
            .enumerate()
            .map(|(position, step_type)| step_of(step_type, &format!("step-{position}")))
            .collect();
        // The exception list is the same length with the rotation reversed, so the two
        // lists are not the same list.
        let on_exception: Vec<Value> = types
            .iter()
            .rev()
            .enumerate()
            .map(|(position, step_type)| step_of(step_type, &format!("rescue-{position}")))
            .collect();

        let created = ok(
            &emulator,
            &case,
            "CreateWorkflow",
            json!({"Steps": steps, "OnExceptionSteps": on_exception}),
        )
        .await;
        let workflow_id = minted(&case, &created, "WorkflowId");
        let body = ok(
            &emulator,
            &case,
            "DescribeWorkflow",
            json!({"WorkflowId": workflow_id}),
        )
        .await;
        let workflow = member_of(&body, "Workflow").clone();

        for (member, expected_types) in [
            ("Steps", types.clone()),
            (
                "OnExceptionSteps",
                types.iter().rev().copied().collect::<Vec<&str>>(),
            ),
        ] {
            let elements = member_of(&workflow, member)
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert_eq!(
                elements.len(),
                length,
                "case {case:?}: expected {member} to report {length} elements, got {elements:?}"
            );
            for (position, element) in elements.iter().enumerate() {
                let actual = member_of(element, discriminator);
                assert_eq!(
                    actual.as_str(),
                    Some(shape_name),
                    "case {case:?}: expected {member}[{position}] to carry \
                     {discriminator}={shape_name:?}, got {element}"
                );
                // The discriminator rides BESIDE the step's own members, which are
                // unchanged by it.
                assert_eq!(
                    member_of(element, "Type").as_str(),
                    Some(expected_types[position]),
                    "case {case:?}: expected {member}[{position}] to keep its submitted \
                     Type, got {element}"
                );
                // It tags the list ELEMENT only: the step's own details structure
                // carries no discriminator of its own.
                let details_member = STEP_TYPES
                    .iter()
                    .find(|(known, _)| *known == expected_types[position])
                    .map(|(_, member)| *member)
                    .expect("every row's step type is a modelled one");
                let details = member_of(element, details_member);
                assert!(
                    member_of(details, discriminator).is_null(),
                    "case {case:?}: expected {member}[{position}].{details_member} to carry no \
                     {discriminator}, got {details}"
                );
            }
        }

        // Nor does the workflow structure itself, nor the response envelope.
        assert!(
            member_of(&workflow, discriminator).is_null(),
            "case {case:?}: expected the workflow to carry no {discriminator}, got {workflow}"
        );
        assert!(
            member_of(&body, discriminator).is_null(),
            "case {case:?}: expected the response to carry no {discriminator}, got {body}"
        );
    }
}
