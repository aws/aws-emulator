// SPDX-License-Identifier: CC0-1.0

//! What `ListThingPrincipals` reports BESIDE the principals of a thing: which of them hold
//! the thing exclusively.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a restJson1
//! request signed for the `iot` signing name, with the principal in the `x-amzn-principal`
//! header the model binds it to — so what is asserted is the JSON an AWS SDK client reads,
//! member for member, and not an internal function's return value.
//!
//! The rule under test: a member the service reports on EVERY answer is always serialized,
//! and a list member with nothing to report carries an empty list rather than being left
//! off, so a client that reads the member unconditionally never sees `null`. For this
//! listing that member is `principalsWithExclusiveAttachment`, derived from the
//! `ThingPrincipalType` each attachment was recorded under
//! (`smithy_query(query="shape",shape="ThingPrincipalType")`: `EXCLUSIVE_THING |
//! NON_EXCLUSIVE_THING`, with `NON_EXCLUSIVE_THING` the value an attach naming none
//! records), and it names exactly the principals of the answered page whose attachment is
//! `EXCLUSIVE_THING`.
//!
//! The tables span what that rule generalises over rather than one recorded request: how
//! many principals are attached (none, one, two, three), which `thingPrincipalType` each
//! attach stated (absent, `NON_EXCLUSIVE_THING`, `EXCLUSIVE_THING` — the whole enum), both
//! forms a thing principal is named in (a certificate ARN and an Amazon Cognito identity),
//! the page size the listing is asked for, and an attachment re-recorded under the other
//! type.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iot_awsiotservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the harness signs for, and the one the emulated ARNs are scoped to.
const REGION: &str = "us-east-1";

/// The `ThingPrincipalType` that says the attachment holds the thing exclusively, and the
/// other value the enum admits — the one an attach that names none is recorded under.
const EXCLUSIVE: &str = "EXCLUSIVE_THING";
const NON_EXCLUSIVE: &str = "NON_EXCLUSIVE_THING";

/// The member this suite is about: reported on every answer of the listing.
const EXCLUSIVE_MEMBER: &str = "principalsWithExclusiveAttachment";

/// Amazon Cognito identities of the `<region>:<id>` form `AttachThingPrincipal$principal`
/// documents ("a certificate ARN (as returned from the CreateCertificate operation) or an
/// Amazon Cognito ID"), which are principals no record of this account has to exist for.
const COGNITO_IDENTITIES: [&str; 2] = [
    "us-east-1:33333333-3333-3333-3333-333333333333",
    "eu-west-1:44444444-4444-4444-4444-444444444444",
];

/// A SigV4 `Authorization` header whose credential scope names the `iot` signing name and
/// the region: the scope is how the router picks the claiming service, and how the service
/// picks the region its ARNs are scoped to. The signature itself is never checked here —
/// the harness turns request authentication off, exactly as an unauthenticated emulator
/// runs.
fn signed_for_iot() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTIOT/20260101/{REGION}/iot/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=0000"
    )
}

/// One response, reduced to what a client branches on.
#[derive(Debug)]
struct Reply {
    status: u16,
    body: Value,
}

impl Reply {
    /// A string member of the body, or `None` when it is absent or not a string.
    fn text(&self, name: &str) -> Option<String> {
        self.body
            .get(name)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// A named list member of the body as strings, or `None` when the member is ABSENT —
    /// which is the difference this suite is about, so it is never flattened to an empty
    /// list.
    fn string_list(&self, name: &str) -> Option<Vec<String>> {
        self.body.get(name).map(|member| {
            member
                .as_array()
                .unwrap_or_else(|| panic!("member {name} must be a list, got {member}"))
                .iter()
                .map(|entry| entry.as_str().unwrap_or_default().to_string())
                .collect()
        })
    }
}

/// The emulator under test: ONE shared engine with the IoT service mounted on it, reached
/// the way the production router reaches it.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn call(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
        headers: &[(&str, &str)],
    ) -> Reply {
        let payload = body.map_or_else(Vec::new, |value| {
            serde_json::to_vec(value).expect("a request body serializes")
        });
        let mut builder = http::Request::builder()
            .method(method)
            .uri(format!("https://iot.{REGION}.amazonaws.com{path}"))
            .header("authorization", signed_for_iot())
            .header("content-type", "application/json");
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let request = builder
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply { status, body }
    }

    /// `CreateThing`: `POST /things/{thingName}`.
    async fn create_thing(&self, case: &str, thing_name: &str) {
        let created = self
            .call("POST", &format!("/things/{thing_name}"), Some(&json!({})), &[])
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: registering thing {thing_name} must succeed, got {created:?}"
        );
    }

    /// `CreateKeysAndCertificate`: `POST /keys-and-certificate`, answered as the
    /// certificate ARN a thing principal is named by.
    async fn create_certificate(&self, case: &str) -> String {
        let issued = self
            .call("POST", "/keys-and-certificate?setAsActive=true", None, &[])
            .await;
        assert_eq!(
            issued.status, 200,
            "case {case:?}: issuing a certificate must succeed, got {issued:?}"
        );
        issued
            .text("certificateArn")
            .unwrap_or_else(|| panic!("case {case:?}: the issued certificate reports an ARN"))
    }

    /// `AttachThingPrincipal`: `PUT /things/{thingName}/principals`, with
    /// `thingPrincipalType` in the query member the model binds it to when the case states
    /// one.
    async fn attach_principal(
        &self,
        case: &str,
        thing_name: &str,
        principal: &str,
        thing_principal_type: Option<&str>,
    ) {
        let path = match thing_principal_type {
            Some(principal_type) => {
                format!("/things/{thing_name}/principals?thingPrincipalType={principal_type}")
            }
            None => format!("/things/{thing_name}/principals"),
        };
        let attached = self
            .call("PUT", &path, None, &[("x-amzn-principal", principal)])
            .await;
        assert_eq!(
            attached.status, 200,
            "case {case:?}: attaching {principal} to {thing_name} as \
             {thing_principal_type:?} must succeed, got {attached:?}"
        );
    }

    /// `DetachThingPrincipal`: `DELETE /things/{thingName}/principals`.
    async fn detach_principal(&self, case: &str, thing_name: &str, principal: &str) {
        let detached = self
            .call(
                "DELETE",
                &format!("/things/{thing_name}/principals"),
                None,
                &[("x-amzn-principal", principal)],
            )
            .await;
        assert_eq!(
            detached.status, 200,
            "case {case:?}: detaching {principal} from {thing_name} must succeed, got \
             {detached:?}"
        );
    }

    /// `ListThingPrincipals`: `GET /things/{thingName}/principals`, optionally for one page
    /// of the listing.
    async fn list_principals(
        &self,
        thing_name: &str,
        max_results: Option<i32>,
        next_token: Option<&str>,
    ) -> Reply {
        let mut query: Vec<String> = Vec::new();
        if let Some(max) = max_results {
            query.push(format!("maxResults={max}"));
        }
        if let Some(token) = next_token {
            query.push(format!("nextToken={token}"));
        }
        let path = if query.is_empty() {
            format!("/things/{thing_name}/principals")
        } else {
            format!("/things/{thing_name}/principals?{}", query.join("&"))
        };
        self.call("GET", &path, None, &[]).await
    }
}

/// Which form a case names its principal in: a certificate this account issues, or an
/// Amazon Cognito identity, which resolves no further than its form.
#[derive(Debug, Clone, Copy)]
enum PrincipalForm {
    Certificate,
    CognitoIdentity(usize),
}

/// One attachment a case sets up: the form the principal is named in and the
/// `thingPrincipalType` the attach states, if any.
#[derive(Debug, Clone, Copy)]
struct Attachment {
    form: PrincipalForm,
    thing_principal_type: Option<&'static str>,
}

impl Attachment {
    const fn new(form: PrincipalForm, thing_principal_type: Option<&'static str>) -> Self {
        Self {
            form,
            thing_principal_type,
        }
    }

    /// Whether the attach this describes is recorded as an EXCLUSIVE one: only an attach
    /// that STATES `EXCLUSIVE_THING` is, since an attach naming no type is recorded
    /// `NON_EXCLUSIVE_THING`.
    fn is_exclusive(&self) -> bool {
        self.thing_principal_type == Some(EXCLUSIVE)
    }
}

/// One row of the listing table.
struct Row {
    case: &'static str,
    thing: &'static str,
    attachments: Vec<Attachment>,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a thing with no principal attached at all",
            thing: "unattached-thing",
            attachments: vec![],
        },
        Row {
            case: "one certificate attached without a thingPrincipalType",
            thing: "default-type-thing",
            attachments: vec![Attachment::new(PrincipalForm::Certificate, None)],
        },
        Row {
            case: "one certificate attached NON_EXCLUSIVE_THING",
            thing: "non-exclusive-thing",
            attachments: vec![Attachment::new(
                PrincipalForm::Certificate,
                Some(NON_EXCLUSIVE),
            )],
        },
        Row {
            case: "one certificate attached EXCLUSIVE_THING",
            thing: "exclusive-thing",
            attachments: vec![Attachment::new(PrincipalForm::Certificate, Some(EXCLUSIVE))],
        },
        Row {
            case: "a Cognito identity attached EXCLUSIVE_THING beside an untyped certificate",
            thing: "mixed-form-thing",
            attachments: vec![
                Attachment::new(PrincipalForm::CognitoIdentity(0), Some(EXCLUSIVE)),
                Attachment::new(PrincipalForm::Certificate, None),
            ],
        },
        Row {
            case: "three principals, two of them exclusively attached",
            thing: "three-principal-thing",
            attachments: vec![
                Attachment::new(PrincipalForm::Certificate, Some(EXCLUSIVE)),
                Attachment::new(PrincipalForm::Certificate, Some(NON_EXCLUSIVE)),
                Attachment::new(PrincipalForm::CognitoIdentity(1), Some(EXCLUSIVE)),
            ],
        },
    ]
}

/// Set a row's thing and attachments up, answering `(principal, exclusive?)` per
/// attachment in the form the listing reports the principal in.
async fn given_attachments(
    emulator: &Emulator,
    case: &str,
    thing: &str,
    attachments: &[Attachment],
) -> Vec<(String, bool)> {
    emulator.create_thing(case, thing).await;
    let mut recorded = Vec::new();
    for attachment in attachments {
        let principal = match attachment.form {
            PrincipalForm::Certificate => emulator.create_certificate(case).await,
            PrincipalForm::CognitoIdentity(index) => COGNITO_IDENTITIES[index].to_string(),
        };
        emulator
            .attach_principal(case, thing, &principal, attachment.thing_principal_type)
            .await;
        recorded.push((principal, attachment.is_exclusive()));
    }
    recorded
}

/// The reported principals of `recorded`, sorted the way the listing orders them.
fn sorted_principals(recorded: &[(String, bool)], exclusive_only: bool) -> Vec<String> {
    let mut principals: Vec<String> = recorded
        .iter()
        .filter(|(_, exclusive)| !exclusive_only || *exclusive)
        .map(|(principal, _)| principal.clone())
        .collect();
    principals.sort();
    principals
}

#[tokio::test]
async fn the_listing_always_reports_which_principals_hold_the_thing_exclusively() {
    let emulator = Emulator::new();

    for Row {
        case,
        thing,
        attachments,
    } in rows()
    {
        let recorded = given_attachments(&emulator, case, thing, &attachments).await;

        let listed = emulator.list_principals(thing, None, None).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: the listing must answer 200, got {listed:?}"
        );
        assert_eq!(
            listed.string_list("principals"),
            Some(sorted_principals(&recorded, false)),
            "case {case:?}: expected every attached principal to be reported, got {listed:?}"
        );
        assert_eq!(
            listed.string_list(EXCLUSIVE_MEMBER),
            Some(sorted_principals(&recorded, true)),
            "case {case:?}: expected {EXCLUSIVE_MEMBER} to name exactly the exclusively \
             attached principals, got {listed:?}"
        );

        // Reading the listing a second time reports the same answer: the member is derived
        // from the relation, and the read writes nothing.
        let again = emulator.list_principals(thing, None, None).await;
        assert_eq!(
            again.body, listed.body,
            "case {case:?}: a second listing must report the same answer, got {again:?}"
        );
    }
}

#[tokio::test]
async fn every_page_of_the_listing_reports_the_member_for_the_principals_it_carries() {
    let emulator = Emulator::new();
    let case = "three principals, the first and the last exclusively attached, one per page";
    let thing = "paged-thing";

    let recorded = given_attachments(
        &emulator,
        case,
        thing,
        &[
            Attachment::new(PrincipalForm::Certificate, Some(EXCLUSIVE)),
            Attachment::new(PrincipalForm::Certificate, None),
            Attachment::new(PrincipalForm::CognitoIdentity(0), Some(EXCLUSIVE)),
        ],
    )
    .await;

    // One principal per page, which is the smallest page `RegistryMaxResults` admits
    // (`range 1..250`), so every page carries exactly one row and the member has to be
    // scoped to it.
    let mut walked: Vec<String> = Vec::new();
    let mut walked_exclusive: Vec<String> = Vec::new();
    let mut next_token: Option<String> = None;
    let mut pages = 0;
    loop {
        let page = emulator
            .list_principals(thing, Some(1), next_token.as_deref())
            .await;
        assert_eq!(
            page.status, 200,
            "case {case:?}: page {pages} must answer 200, got {page:?}"
        );
        let principals = page
            .string_list("principals")
            .unwrap_or_else(|| panic!("case {case:?}: page {pages} must report principals"));
        let exclusive = page
            .string_list(EXCLUSIVE_MEMBER)
            .unwrap_or_else(|| panic!("case {case:?}: page {pages} must report {EXCLUSIVE_MEMBER}"));
        assert!(
            exclusive.iter().all(|principal| principals.contains(principal)),
            "case {case:?}: page {pages} must only report {EXCLUSIVE_MEMBER} entries it also \
             reports as principals, got {page:?}"
        );
        let expected_on_page: Vec<String> = principals
            .iter()
            .filter(|principal| {
                recorded
                    .iter()
                    .any(|(recorded, is_exclusive)| recorded == *principal && *is_exclusive)
            })
            .cloned()
            .collect();
        assert_eq!(
            exclusive, expected_on_page,
            "case {case:?}: page {pages} must report the exclusive attachments of its own \
             principals, got {page:?}"
        );
        walked.extend(principals);
        walked_exclusive.extend(exclusive);
        pages += 1;
        next_token = page.text("nextToken");
        if next_token.is_none() {
            break;
        }
        assert!(
            pages <= recorded.len(),
            "case {case:?}: the walk must end within {} pages, got {pages}",
            recorded.len()
        );
    }

    assert_eq!(
        walked,
        sorted_principals(&recorded, false),
        "case {case:?}: the pages together must report every attached principal"
    );
    assert_eq!(
        walked_exclusive,
        sorted_principals(&recorded, true),
        "case {case:?}: the pages together must report every exclusive attachment"
    );
}

#[tokio::test]
async fn the_member_follows_the_type_the_attachment_is_recorded_under() {
    let emulator = Emulator::new();
    let case = "an attachment re-recorded under the other type, then detached";
    let thing = "retyped-thing";

    emulator.create_thing(case, thing).await;
    let principal = emulator.create_certificate(case).await;

    // Each step states the type the attachment is recorded under, and the listing reports
    // the member off that record — never off the first attach.
    for (step, thing_principal_type, exclusive) in [
        ("attached NON_EXCLUSIVE_THING", Some(NON_EXCLUSIVE), false),
        ("re-attached EXCLUSIVE_THING", Some(EXCLUSIVE), true),
        ("re-attached without a type", None, false),
        ("re-attached EXCLUSIVE_THING again", Some(EXCLUSIVE), true),
    ] {
        emulator
            .attach_principal(case, thing, &principal, thing_principal_type)
            .await;
        let listed = emulator.list_principals(thing, None, None).await;
        assert_eq!(
            listed.string_list("principals"),
            Some(vec![principal.clone()]),
            "case {case:?} / {step}: the one attachment must be reported once, got {listed:?}"
        );
        let expected = if exclusive {
            vec![principal.clone()]
        } else {
            Vec::new()
        };
        assert_eq!(
            listed.string_list(EXCLUSIVE_MEMBER),
            Some(expected),
            "case {case:?} / {step}: expected {EXCLUSIVE_MEMBER} to follow the recorded \
             type, got {listed:?}"
        );
    }

    // Detached, the thing has no principal at all: both members are reported, each as an
    // empty list.
    emulator.detach_principal(case, thing, &principal).await;
    let listed = emulator.list_principals(thing, None, None).await;
    assert_eq!(
        listed.string_list("principals"),
        Some(Vec::new()),
        "case {case:?}: a detached thing reports no principals, got {listed:?}"
    );
    assert_eq!(
        listed.string_list(EXCLUSIVE_MEMBER),
        Some(Vec::new()),
        "case {case:?}: a detached thing reports {EXCLUSIVE_MEMBER} as an empty list, got \
         {listed:?}"
    );
}
