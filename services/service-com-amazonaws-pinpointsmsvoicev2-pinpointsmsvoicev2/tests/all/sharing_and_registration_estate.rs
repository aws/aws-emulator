// SPDX-License-Identifier: CC0-1.0

//! The SHARING and REGISTRATION estate of this service, end to end over the wire: the
//! opt-out list, the RCS agent, the registration and the registration attachment a caller
//! creates, and the resource-based policy a caller attaches to a resource it holds.
//!
//! Every request goes through [`MultiServiceRouter::dispatch`] as `awsJson1_0` — `POST /`
//! with `X-Amz-Target: PinpointSMSVoiceV2.<Op>` — so these tests exercise routing,
//! deserialization, constraint validation, the handler and serialization, exactly as a real
//! SDK client would reach them.
//!
//! The rule under test: **a create of one of these resources PERSISTS it under the identity
//! this service decides — the name the caller gave it, or the identifier this service minted
//! — every read reports it out of that record, and a delete removes it so a later read is
//! this service's own `ResourceNotFoundException`.** The rows of the round-trip table differ
//! along what that rule generalises over: which resource is created (four kinds, two
//! caller-named and two service-identified), how a later call ADDRESSES it (by name or id,
//! and by the ARN this service built around it), and how many tags the declaration carries.
//!
//! The policy table pins the second half: **a resource-based policy is held AGAINST the
//! resource its ARN names, so a put on an ARN this account holds nothing for is a
//! not-found, a read of a resource that carries no policy is the empty document, and a
//! delete detaches the one that was attached.** Its rows differ along the KIND of resource
//! the policy is attached to, which is the model's own `ResourceType` set.
//!
//! Its harness is this suite's own: the sibling suites' helpers are private to their
//! modules, and a test target shares nothing it does not export.

use std::sync::Arc;
use std::time::{Duration, Instant};

use service_com_amazonaws_pinpointsmsvoicev2_pinpointsmsvoicev2 as sms_voice;
use smithy_server_runtime::MultiServiceRouter;

/// The region these tests sign for, so the ARNs under assertion are not the default one.
const REGION: &str = "ap-southeast-2";

/// How long this suite is willing to wait for a clock-owed transition to come due, and how
/// often it looks again. Generous: the suite waits for the CONDITION (the attachment's
/// status) rather than for a fixed time.
const SETTLE_DEADLINE: Duration = Duration::from_secs(30);
const LOOK_AGAIN_AFTER: Duration = Duration::from_millis(50);

/// The `AttachmentStatus` a created attachment is in, and the one its upload finishes in.
const UPLOAD_IN_PROGRESS: &str = "UPLOAD_IN_PROGRESS";
const UPLOAD_COMPLETE: &str = "UPLOAD_COMPLETE";

/// The document `GetResourcePolicy` reports for a resource that carries no policy.
const EMPTY_POLICY: &str = "{}";

/// One emulator: the shared engine plus a router with only this service mounted.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These tests assert behaviour, not signatures; the emulator verifies signatures
        // itself and a service never checks them.
        router.set_request_authentication(false);
        router.register_service(sms_voice::EmulatedService::new(engine));
        Self { router }
    }

    /// Call one operation and answer `(http status, parsed JSON body)`.
    async fn call(&self, operation: &str, body: serde_json::Value) -> (u16, serde_json::Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.0")
            .header("x-amz-target", format!("PinpointSMSVoiceV2.{operation}"))
            // The SigV4 credential scope is what routes the request to this service and
            // what names the region its ARNs carry.
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/sms-voice/aws4_request, \
                     SignedHeaders=host;x-amz-date, Signature=0123456789abcdef"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(&body).expect("the request body serializes"),
            ))
            .expect("a valid request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(serde_json::Value::Null)
        };
        (status, parsed)
    }

    /// Call one operation and require HTTP 200, reporting the body when it is not.
    async fn ok(&self, operation: &str, body: serde_json::Value) -> serde_json::Value {
        let (status, parsed) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "{operation}: expected HTTP 200, got {status} with {parsed}"
        );
        parsed
    }
}

/// The wire error code of a failed call (`__type`, with any namespace prefix dropped).
fn error_code(body: &serde_json::Value) -> String {
    let raw = body
        .get("__type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("<no __type>");
    raw.rsplit('#').next().unwrap_or(raw).to_string()
}

/// One string member of a body, or a panic naming what was missing.
fn member(body: &serde_json::Value, name: &str) -> String {
    body.get(name)
        .and_then(serde_json::Value::as_str)
        .unwrap_or_else(|| panic!("expected a string member {name} in {body}"))
        .to_string()
}

/// This service's not-found sentence: a restatement of the exception's own structured
/// members, as every other not-found this service raises words it.
fn not_found_sentence(resource_type: &str, resource_id: &str) -> String {
    format!("Resource Not Found - ResourceType=\"{resource_type}\" ResourceId=\"{resource_id}\"")
}

/// Assert one not-found refusal: the status, the wire error code and the structured members.
fn assert_not_found(
    case: &str,
    status: u16,
    body: &serde_json::Value,
    resource_type: &str,
    resource_id: &str,
) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(body),
        "ResourceNotFoundException",
        "case {case:?}: got {body}"
    );
    assert_eq!(
        member(body, "ResourceType"),
        resource_type,
        "case {case:?}: got {body}"
    );
    assert_eq!(
        member(body, "ResourceId"),
        resource_id,
        "case {case:?}: got {body}"
    );
    assert_eq!(
        member(body, "Message"),
        not_found_sentence(resource_type, resource_id),
        "case {case:?}: got {body}"
    );
}

/// Which of the four resources one row of the round-trip table creates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// Named by the caller; `opt-out-list` in the model's own `ResourceType` set.
    OptOutList,
    /// Identified by what `CreateRcsAgent` minted.
    RcsAgent,
    /// Identified by what `CreateRegistration` minted.
    Registration,
    /// Identified by what `CreateRegistrationAttachment` minted.
    RegistrationAttachment,
}

impl Kind {
    /// The model's `ResourceType` for this resource, which its refusals name
    /// (`smithy_query(query="shape",shape="ResourceType")`).
    fn resource_type(self) -> &'static str {
        match self {
            Self::OptOutList => "opt-out-list",
            Self::RcsAgent => "rcs-agent",
            Self::Registration => "registration",
            Self::RegistrationAttachment => "registration-attachment",
        }
    }

    /// The operations that create, read and delete one of them.
    fn operations(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Self::OptOutList => (
                "CreateOptOutList",
                "DescribeOptOutLists",
                "DeleteOptOutList",
            ),
            Self::RcsAgent => ("CreateRcsAgent", "DescribeRcsAgents", "DeleteRcsAgent"),
            Self::Registration => (
                "CreateRegistration",
                "DescribeRegistrations",
                "DeleteRegistration",
            ),
            Self::RegistrationAttachment => (
                "CreateRegistrationAttachment",
                "DescribeRegistrationAttachments",
                "DeleteRegistrationAttachment",
            ),
        }
    }

    /// The members a create answers the resource's identity and ARN in, and the member a
    /// later call addresses one by, both on the singular calls and as the LIST a read
    /// selects with.
    fn members(self) -> ResourceMembers {
        match self {
            Self::OptOutList => ResourceMembers {
                id: "OptOutListName",
                arn: "OptOutListArn",
                address: "OptOutListName",
                selection: "OptOutListNames",
                reported: "OptOutLists",
            },
            Self::RcsAgent => ResourceMembers {
                id: "RcsAgentId",
                arn: "RcsAgentArn",
                address: "RcsAgentId",
                selection: "RcsAgentIds",
                reported: "RcsAgents",
            },
            Self::Registration => ResourceMembers {
                id: "RegistrationId",
                arn: "RegistrationArn",
                address: "RegistrationId",
                selection: "RegistrationIds",
                reported: "Registrations",
            },
            Self::RegistrationAttachment => ResourceMembers {
                id: "RegistrationAttachmentId",
                arn: "RegistrationAttachmentArn",
                address: "RegistrationAttachmentId",
                selection: "RegistrationAttachmentIds",
                reported: "RegistrationAttachments",
            },
        }
    }
}

/// The members one resource's identity travels in, per [`Kind::members`].
struct ResourceMembers {
    id: &'static str,
    arn: &'static str,
    address: &'static str,
    selection: &'static str,
    reported: &'static str,
}

/// One row of [`a_create_persists_the_resource_and_a_delete_takes_it_away`].
struct RoundTrip {
    /// What this row is called in every assertion message.
    case: &'static str,
    /// Which resource the row creates.
    kind: Kind,
    /// Address the resource by the ARN this service built rather than by its identifier.
    by_arn: bool,
    /// The tags the create carries.
    tags: &'static [(&'static str, &'static str)],
}

/// The create request one row makes, beside the tags it carries.
fn create_request(row: &RoundTrip) -> serde_json::Value {
    let tags: Vec<serde_json::Value> = row
        .tags
        .iter()
        .map(|(key, value)| serde_json::json!({"Key": key, "Value": value}))
        .collect();
    let mut request = match row.kind {
        // `OptOutListName` is `^[A-Za-z0-9_-]+$`, so the row's own name is spelled that way.
        Kind::OptOutList => serde_json::json!({"OptOutListName": format!("list-{}", row.case.len())}),
        Kind::RcsAgent => serde_json::json!({"DeletionProtectionEnabled": false}),
        // `RegistrationType` is `^[A-Za-z0-9_]+$`; the set of types a real account may use
        // is `DescribeRegistrationTypeDefinitions`', which this emulator serves none of, so
        // the create takes the type the declaration states.
        Kind::Registration => serde_json::json!({"RegistrationType": "US_TOLL_FREE_REGISTRATION"}),
        Kind::RegistrationAttachment => {
            serde_json::json!({"AttachmentUrl": "https://example.invalid/evidence.pdf"})
        }
    };
    if !tags.is_empty() {
        request["Tags"] = serde_json::Value::Array(tags);
    }
    request
}

#[tokio::test]
async fn a_create_persists_the_resource_and_a_delete_takes_it_away() {
    // The rows differ along which resource is created, how a later call addresses it, and
    // how many tags the create carries.
    let rows = [
        RoundTrip {
            case: "an opt-out list, addressed by its name, with no tags",
            kind: Kind::OptOutList,
            by_arn: false,
            tags: &[],
        },
        RoundTrip {
            case: "an opt-out list addressed by its ARN, with two tags",
            kind: Kind::OptOutList,
            by_arn: true,
            tags: &[("owner", "sharing-suite"), ("stage", "test")],
        },
        RoundTrip {
            case: "an RCS agent, addressed by the id this service minted",
            kind: Kind::RcsAgent,
            by_arn: false,
            tags: &[("owner", "sharing-suite")],
        },
        RoundTrip {
            case: "an RCS agent addressed by its ARN",
            kind: Kind::RcsAgent,
            by_arn: true,
            tags: &[],
        },
        RoundTrip {
            case: "a registration, addressed by the id this service minted",
            kind: Kind::Registration,
            by_arn: false,
            tags: &[("owner", "sharing-suite")],
        },
        RoundTrip {
            case: "a registration attachment addressed by its ARN",
            kind: Kind::RegistrationAttachment,
            by_arn: true,
            tags: &[("owner", "sharing-suite")],
        },
    ];

    for row in &rows {
        let case = row.case;
        // One account per row, so a row's assertions are about its own resource alone.
        let emulator = Emulator::new();
        let members = row.kind.members();
        let (create, read, delete) = row.kind.operations();

        let created = emulator.ok(create, create_request(row)).await;
        let id = member(&created, members.id);
        let arn = member(&created, members.arn);
        assert!(
            arn.starts_with("arn:")
                && arn.contains(REGION)
                && arn.ends_with(&format!("{}/{id}", row.kind.resource_type())),
            "case {case:?}: expected an ARN of this service naming {id:?} under \
             {:?} in {REGION}, got {arn:?}",
            row.kind.resource_type()
        );

        // The tags the create carries come back off the record, and a resource with none
        // carries no `Tags` member at all.
        let reported_tags = created.get("Tags").and_then(serde_json::Value::as_array);
        assert_eq!(
            reported_tags.map(Vec::len).unwrap_or(0),
            row.tags.len(),
            "case {case:?}: expected {} tags, got {created}",
            row.tags.len()
        );

        // The resource is readable back by the identity the create answered, through the
        // read's own selection member — which takes the id or the ARN this service built.
        let handle = if row.by_arn { arn.clone() } else { id.clone() };
        let described = emulator
            .ok(
                read,
                serde_json::json!({ members.selection: [handle.clone()] }),
            )
            .await;
        let records = described
            .get(members.reported)
            .and_then(serde_json::Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: no {} in {described}", members.reported));
        assert_eq!(
            records.len(),
            1,
            "case {case:?}: expected exactly the one resource asked about, got {described}"
        );
        assert_eq!(
            (
                member(&records[0], members.id),
                member(&records[0], members.arn)
            ),
            (id.clone(), arn.clone()),
            "case {case:?}: expected the read to report the created identity, got {described}"
        );

        // A delete removes it and reports the record it removed...
        let removed = emulator
            .ok(delete, serde_json::json!({ members.address: handle.clone() }))
            .await;
        assert_eq!(
            (member(&removed, members.id), member(&removed, members.arn)),
            (id.clone(), arn.clone()),
            "case {case:?}: expected the delete to report the stored record, got {removed}"
        );

        // ...so a later read of it is this service's own not-found refusal, and the whole
        // account reports nothing.
        let (status, refused) = emulator
            .call(read, serde_json::json!({ members.selection: [handle.clone()] }))
            .await;
        assert_not_found(
            case,
            status,
            &refused,
            row.kind.resource_type(),
            // A not-found names the resource as the request spelled it, which for an ARN is
            // the identifier inside it.
            &id,
        );
        let empty = emulator.ok(read, serde_json::json!({})).await;
        assert_eq!(
            empty
                .get(members.reported)
                .and_then(serde_json::Value::as_array)
                .map(Vec::len)
                .unwrap_or(0),
            0,
            "case {case:?}: expected the account to hold none after the delete, got {empty}"
        );
    }
}

#[tokio::test]
async fn one_client_token_creates_one_resource() {
    // The rule the `@idempotencyToken` of each create carries: the same token must not mint
    // a second resource, whichever of the creates it is used on.
    for kind in [
        Kind::OptOutList,
        Kind::RcsAgent,
        Kind::Registration,
        Kind::RegistrationAttachment,
    ] {
        let case = format!("{kind:?} created twice under one ClientToken");
        let emulator = Emulator::new();
        let members = kind.members();
        let (create, read, _) = kind.operations();
        let row = RoundTrip {
            case: "idempotent",
            kind,
            by_arn: false,
            tags: &[],
        };
        let mut request = create_request(&row);
        request["ClientToken"] = serde_json::Value::String("one-token".to_string());

        let first = emulator.ok(create, request.clone()).await;
        let second = emulator.ok(create, request).await;
        assert_eq!(
            member(&first, members.id),
            member(&second, members.id),
            "case {case:?}: expected the replay to answer the first resource, got {second}"
        );
        let described = emulator.ok(read, serde_json::json!({})).await;
        assert_eq!(
            described
                .get(members.reported)
                .and_then(serde_json::Value::as_array)
                .map(Vec::len)
                .unwrap_or(0),
            1,
            "case {case:?}: expected one resource in the account, got {described}"
        );
    }
}

#[tokio::test]
async fn an_attachment_upload_finishes_on_its_own() {
    // `CreateRegistrationAttachment` answers `UPLOAD_IN_PROGRESS` — the document is being
    // uploaded — and the attachment reaches `UPLOAD_COMPLETE` without anybody asking for it.
    let case = "an attachment supplied inline";
    let emulator = Emulator::new();
    let created = emulator
        .ok(
            "CreateRegistrationAttachment",
            // `AttachmentBody` is a blob, which awsJson carries as base64 text.
            serde_json::json!({"AttachmentBody": "ZXZpZGVuY2U="}),
        )
        .await;
    let id = member(&created, "RegistrationAttachmentId");
    assert_eq!(
        member(&created, "AttachmentStatus"),
        UPLOAD_IN_PROGRESS,
        "case {case:?}: expected the create to answer an upload in progress, got {created}"
    );

    // Wait for the CONDITION, looking again until the status has moved or a generous
    // deadline passes.
    let deadline = Instant::now() + SETTLE_DEADLINE;
    loop {
        let described = emulator
            .ok(
                "DescribeRegistrationAttachments",
                serde_json::json!({"RegistrationAttachmentIds": [id.clone()]}),
            )
            .await;
        let reported = described
            .get("RegistrationAttachments")
            .and_then(serde_json::Value::as_array)
            .and_then(|records| records.first().cloned())
            .unwrap_or_else(|| panic!("case {case:?}: no attachment in {described}"));
        let status = member(&reported, "AttachmentStatus");
        if status == UPLOAD_COMPLETE {
            // The attachment is still the same one, and the URL member is absent for a
            // document that came inline.
            assert_eq!(
                member(&reported, "RegistrationAttachmentId"),
                id,
                "case {case:?}: expected the same attachment, got {reported}"
            );
            assert!(
                reported.get("AttachmentUrl").is_none(),
                "case {case:?}: expected no URL for an inline document, got {reported}"
            );
            return;
        }
        assert_eq!(
            status, UPLOAD_IN_PROGRESS,
            "case {case:?}: expected the upload to be in progress or complete, got {reported}"
        );
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the upload to finish, it is still {status:?}"
        );
        tokio::time::sleep(LOOK_AGAIN_AFTER).await;
    }
}

#[tokio::test]
async fn an_attachment_carries_exactly_one_document() {
    // The document is supplied EITHER inline or from a URL: both at once is two documents
    // for one attachment, and neither is no document at all.
    let cases: [(&str, serde_json::Value, &str); 2] = [
        (
            "both the body and the URL",
            serde_json::json!({
                "AttachmentBody": "ZXZpZGVuY2U=",
                "AttachmentUrl": "https://example.invalid/evidence.pdf",
            }),
            "PARAMETERS_CANNOT_BE_USED_TOGETHER",
        ),
        (
            "neither the body nor the URL",
            serde_json::json!({}),
            "MISSING_PARAMETER",
        ),
    ];
    let emulator = Emulator::new();
    for (case, request, reason) in cases {
        let (status, refused) = emulator.call("CreateRegistrationAttachment", request).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected a validation refusal, got {status} with {refused}"
        );
        assert_eq!(
            member(&refused, "Reason"),
            reason,
            "case {case:?}: got {refused}"
        );
        let described = emulator
            .ok("DescribeRegistrationAttachments", serde_json::json!({}))
            .await;
        assert_eq!(
            described
                .get("RegistrationAttachments")
                .and_then(serde_json::Value::as_array)
                .map(Vec::len)
                .unwrap_or(0),
            0,
            "case {case:?}: expected the refusal to create nothing, got {described}"
        );
    }
}

/// One row of [`a_resource_policy_is_held_against_the_resource_its_arn_names`]: which kind of
/// resource the policy is attached to.
struct PolicyRow {
    case: &'static str,
    /// The model's `ResourceType` for the resource the policy is attached to.
    resource_type: &'static str,
    /// The operation that creates it, the request that creates it, and the member its ARN is
    /// answered in.
    create: &'static str,
    request: fn() -> serde_json::Value,
    arn_member: &'static str,
    /// The operation that deletes it, and the member it is addressed by.
    delete: &'static str,
    address_member: &'static str,
    /// The member the delete takes: the resource's own name or minted id.
    id_member: &'static str,
}

#[tokio::test]
async fn a_resource_policy_is_held_against_the_resource_its_arn_names() {
    // The rows differ along the KIND of resource the policy is attached to — three of the
    // model's own `ResourceType` values, one caller-named and one service-identified among
    // them, and a configuration set the issue's own templates attach one to.
    let rows = [
        PolicyRow {
            case: "a policy on a configuration set",
            resource_type: "configuration-set",
            create: "CreateConfigurationSet",
            request: || serde_json::json!({"ConfigurationSetName": "policy-set"}),
            arn_member: "ConfigurationSetArn",
            delete: "DeleteConfigurationSet",
            address_member: "ConfigurationSetName",
            id_member: "ConfigurationSetName",
        },
        PolicyRow {
            case: "a policy on an opt-out list",
            resource_type: "opt-out-list",
            create: "CreateOptOutList",
            request: || serde_json::json!({"OptOutListName": "policy-list"}),
            arn_member: "OptOutListArn",
            delete: "DeleteOptOutList",
            address_member: "OptOutListName",
            id_member: "OptOutListName",
        },
        PolicyRow {
            case: "a policy on a protect configuration",
            resource_type: "protect-configuration",
            create: "CreateProtectConfiguration",
            request: || serde_json::json!({"DeletionProtectionEnabled": false}),
            arn_member: "ProtectConfigurationArn",
            delete: "DeleteProtectConfiguration",
            address_member: "ProtectConfigurationId",
            id_member: "ProtectConfigurationId",
        },
    ];

    for row in &rows {
        let case = row.case;
        let emulator = Emulator::new();
        let created = emulator.ok(row.create, (row.request)()).await;
        let arn = member(&created, row.arn_member);
        let id = member(&created, row.id_member);

        // A resource that carries NO policy reads back the empty document rather than a
        // refusal: the resource itself exists.
        let read = emulator
            .ok("GetResourcePolicy", serde_json::json!({"ResourceArn": arn}))
            .await;
        assert_eq!(
            member(&read, "Policy"),
            EMPTY_POLICY,
            "case {case:?}: expected the empty document for a resource with no policy, got {read}"
        );

        // The document a put attaches is the document a read answers, byte for byte.
        let document = format!(
            "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Sid\":\"Share\",\"Effect\":\"Allow\",\
             \"Principal\":{{\"AWS\":\"123456789012\"}},\"Action\":\"sms-voice:SendTextMessage\",\
             \"Resource\":\"{arn}\"}}]}}"
        );
        let put = emulator
            .ok(
                "PutResourcePolicy",
                serde_json::json!({"ResourceArn": arn, "Policy": document}),
            )
            .await;
        assert_eq!(
            member(&put, "Policy"),
            document,
            "case {case:?}: expected the put to answer the stored document, got {put}"
        );
        let read = emulator
            .ok("GetResourcePolicy", serde_json::json!({"ResourceArn": arn}))
            .await;
        assert_eq!(
            (member(&read, "ResourceArn"), member(&read, "Policy")),
            (arn.clone(), document.clone()),
            "case {case:?}: expected the read to answer the stored document, got {read}"
        );

        // A delete detaches it, and the read is the empty document again.
        let detached = emulator
            .ok(
                "DeleteResourcePolicy",
                serde_json::json!({"ResourceArn": arn}),
            )
            .await;
        assert_eq!(
            member(&detached, "Policy"),
            document,
            "case {case:?}: expected the delete to answer the document it detached, got {detached}"
        );
        let read = emulator
            .ok("GetResourcePolicy", serde_json::json!({"ResourceArn": arn}))
            .await;
        assert_eq!(
            member(&read, "Policy"),
            EMPTY_POLICY,
            "case {case:?}: expected the empty document once the policy is detached, got {read}"
        );

        // Once the RESOURCE is gone, a policy call about it is this service's own not-found:
        // there is nothing for a policy to be held against.
        emulator
            .ok(row.delete, serde_json::json!({ row.address_member: id }))
            .await;
        let id = member(&created, row.id_member);
        let (status, refused) = emulator
            .call("GetResourcePolicy", serde_json::json!({"ResourceArn": arn}))
            .await;
        assert_not_found(case, status, &refused, row.resource_type, &id);
    }
}

#[tokio::test]
async fn a_policy_on_a_resource_nobody_created_is_a_not_found() {
    // Every policy call is about a resource: an ARN naming one this account holds nothing
    // for names no resource at all, whichever of the three calls asks.
    let emulator = Emulator::new();
    let arn = format!("arn:aws:sms-voice:{REGION}:000000000000:opt-out-list/never-created");
    for operation in [
        "PutResourcePolicy",
        "GetResourcePolicy",
        "DeleteResourcePolicy",
    ] {
        let mut request = serde_json::json!({"ResourceArn": arn});
        if operation == "PutResourcePolicy" {
            request["Policy"] = serde_json::Value::String("{\"Statement\":[]}".to_string());
        }
        let (status, refused) = emulator.call(operation, request).await;
        assert_not_found(
            operation,
            status,
            &refused,
            "opt-out-list",
            "never-created",
        );
    }
}
