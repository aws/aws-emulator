// SPDX-License-Identifier: CC0-1.0

//! The rule that binds a cache behaviour's LAMBDA@EDGE ASSOCIATION to an execution role the
//! EDGE can assume, end to end over HTTP.
//!
//! CloudFront invokes the function version an association names by assuming that function's
//! execution role as the edge principal (`edgelambda.amazonaws.com`) beside the principal
//! Lambda itself assumes it as (`lambda.amazonaws.com`), so a configuration whose
//! association resolves to a role whose trust policy withholds either assume is refused with
//! `InvalidLambdaFunctionAssociation` (HTTP 400), the refusal names the ROLE the caller has
//! to fix, and NOTHING is created or changed. The same role, once its trust policy admits
//! both principals, carries the very same association.
//!
//! Every fact the rule needs is staged through the SERVICES that own it, on one router over
//! one engine: the role through IAM's `CreateRole` / `UpdateAssumeRolePolicy`, and the
//! function and its published version through Lambda's `CreateFunction` / `PublishVersion`.
//! So the execution role the handler resolves is the one a client really created, and the
//! trust document it reads is the one IAM really holds.
//!
//! What the rows span: a role that trusts only Lambda, only the edge, both (listed in one
//! statement, spread over two, and under an action wildcard), both with an explicit deny
//! beside the grant, a document that grants nothing, and a role that trusts another service
//! altogether; an association whose function Lambda holds nothing under and one whose
//! execution role IAM holds no record of, neither of which is evidence about the association;
//! the association carried by the DEFAULT cache behaviour and by an ORDERED one; and both
//! create paths. The update path, and the control that a role repaired with
//! `UpdateAssumeRolePolicy` carries the same version ARN, are the two tests that follow.

use std::sync::Arc;

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::MAX_ITEMS;
use smithy_server_runtime::{EmbeddedEmulator, MultiServiceRouter};

use crate::connection_mode_immutable::get_distribution_config;
use crate::distribution_lifecycle::{
    create_distribution, list_distributions, update_distribution, Answer, ACCOUNT_ID,
};
use crate::lambda_edge_association::{
    config_xml, create_distribution_with_tags, reported_arns, Association, Behavior,
};

/// The Region every function below lives in, which is the Region a Lambda@Edge function has
/// to be created in.
const REGION: &str = "us-east-1";

/// The fixed part of this refusal's message, spelled out here rather than imported from the
/// implementation: it is CloudFront's own text for this error class, so a constant edited to
/// some other wording fails these rows. The ARN of the resolved execution role follows it.
const TRUST_REFUSAL_PREFIX: &str = concat!(
    "The function execution role must be assumable with edgelambda.amazonaws.com as well ",
    "as lambda.amazonaws.com principals. Update the IAM role and try again. Role: "
);

/// The whole message the refusal of an association whose role withholds the edge's assume
/// has to carry.
fn refusal_message(role_arn: &str) -> String {
    format!("{TRUST_REFUSAL_PREFIX}{role_arn}")
}

// ---------------------------------------------------------------------------
// The emulator: CloudFront plus the two services the association resolves against
// ---------------------------------------------------------------------------

/// CloudFront, Lambda and IAM over ONE shared engine, with a callback into the same router so
/// this suite stages the function and the role through their own APIs — the very records the
/// handler reads them from.
struct Emulator {
    router: MultiServiceRouter,
    callback: EmbeddedEmulator,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_cloudfront_cloudfront2020_05_31::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(
            service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        let callback = EmbeddedEmulator::from_router(router.clone());
        Self { router, callback }
    }

    /// The IAM client, built by the one shared constructor over the in-process callback.
    fn iam(&self) -> aws_sdk_iam::Client {
        service_common::iam_client::iam_client_through(&self.callback, REGION)
    }

    /// The Lambda client, over the same in-process callback.
    fn lambda(&self) -> aws_sdk_lambda::Client {
        let config = aws_sdk_lambda::Config::builder()
            .behavior_version(aws_sdk_lambda::config::BehaviorVersion::latest())
            .region(aws_sdk_lambda::config::Region::new(REGION))
            .endpoint_url(format!("https://lambda.{REGION}.amazonaws.com"))
            .credentials_provider(self.callback.aws_credentials_provider())
            .http_client(self.callback.http_client())
            .retry_config(aws_sdk_lambda::config::retry::RetryConfig::disabled())
            .timeout_config(aws_sdk_lambda::config::timeout::TimeoutConfig::disabled())
            .identity_cache(aws_sdk_lambda::config::IdentityCache::no_cache())
            .stalled_stream_protection(
                aws_sdk_lambda::config::StalledStreamProtectionConfig::disabled(),
            )
            .build();
        aws_sdk_lambda::Client::from_conf(config)
    }

    /// `CreateRole` with `trust` as its `AssumeRolePolicyDocument`, and the role ARN IAM
    /// answered with.
    async fn create_role(&self, case: &str, name: &str, trust: &str) -> String {
        let created = self
            .iam()
            .create_role()
            .role_name(name)
            .assume_role_policy_document(trust)
            .send()
            .await
            .unwrap_or_else(|error| panic!("case {case}: creating the role {name}: {error}"));
        created
            .role()
            .and_then(|role| {
                let arn = role.arn();
                (!arn.is_empty()).then(|| arn.to_string())
            })
            .unwrap_or_else(|| panic!("case {case}: the created role {name} has an ARN"))
    }

    /// `UpdateAssumeRolePolicy`: the call a client repairs a role's trust policy with.
    async fn update_trust(&self, case: &str, name: &str, trust: &str) {
        self.iam()
            .update_assume_role_policy()
            .role_name(name)
            .policy_document(trust)
            .send()
            .await
            .unwrap_or_else(|error| panic!("case {case}: updating the trust of {name}: {error}"));
    }

    /// `CreateFunction` under `role_arn`, then `PublishVersion`, and the version ARN Lambda
    /// answered with.
    async fn publish_function(&self, case: &str, name: &str, role_arn: &str) -> String {
        let code = aws_sdk_lambda::types::FunctionCode::builder()
            .zip_file(aws_smithy_types::Blob::new(b"edge-handler".to_vec()))
            .build();
        self.lambda()
            .create_function()
            .function_name(name)
            .role(role_arn)
            .runtime(aws_sdk_lambda::types::Runtime::Python312)
            .handler("index.handler")
            .code(code)
            .send()
            .await
            .unwrap_or_else(|error| panic!("case {case}: creating the function {name}: {error}"));
        let published = self
            .lambda()
            .publish_version()
            .function_name(name)
            .send()
            .await
            .unwrap_or_else(|error| panic!("case {case}: publishing {name}: {error}"));
        published
            .function_arn()
            .map(ToString::to_string)
            .unwrap_or_else(|| panic!("case {case}: the published version has an ARN"))
    }
}

// ---------------------------------------------------------------------------
// The trust documents, and the rows that stage them
// ---------------------------------------------------------------------------

/// A trust policy whose single `Allow` names `services` (one JSON string or a list of them)
/// as the principals of `action`.
fn allow(services: &str, action: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
         \"Principal\":{{\"Service\":{services}}},\"Action\":\"{action}\"}}]}}"
    )
}

/// A trust policy carrying one statement per `(effect, service)` pair, each over
/// `sts:AssumeRole`.
fn statements(entries: &[(&str, &str)]) -> String {
    let body: Vec<String> = entries
        .iter()
        .map(|(effect, service)| {
            format!(
                "{{\"Effect\":\"{effect}\",\"Principal\":{{\"Service\":\"{service}\"}},\
                 \"Action\":\"sts:AssumeRole\"}}"
            )
        })
        .collect();
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{}]}}",
        body.join(",")
    )
}

/// What this suite stages in Lambda and IAM for one row, before CloudFront is asked about the
/// association.
enum Stage {
    /// A function running as a role of this account, a published version of it, and this
    /// trust policy on that role.
    ///
    /// The role is created admitting both principals and then SET to the row's document,
    /// because Lambda refuses to create a function whose execution role withholds Lambda's
    /// own assume — so a row whose document does is staged the way a client that edits a
    /// trust policy stages it, with `UpdateAssumeRolePolicy` after the function exists.
    Role(String),
    /// A function running as a role ARN this account's IAM holds no record of, and a
    /// published version: the ordinary way an execution role exists, which is evidence about
    /// this emulator's IAM rather than about the association.
    RoleNotHeld,
    /// Nothing at all: the ARN names a published version of a function Lambda holds nothing
    /// under.
    Nothing,
}

/// One row: what is staged for the association, and whether CloudFront stores a configuration
/// carrying it.
struct Case {
    case: &'static str,
    stage: Stage,
    /// The version ARN the cache behaviour associates. Its function name is what this suite
    /// stages, and its version is the one `PublishVersion` mints first.
    arn: &'static str,
    accepted: bool,
}

/// The two principals a Lambda@Edge execution role has to admit.
const LAMBDA: &str = "lambda.amazonaws.com";
const EDGE_LAMBDA: &str = "edgelambda.amazonaws.com";

fn cases() -> Vec<Case> {
    vec![
        Case {
            case: "trusts-lambda-only",
            stage: Stage::Role(allow(&format!("\"{LAMBDA}\""), "sts:AssumeRole")),
            arn: "arn:aws:lambda:us-east-1:000000000000:function:edge-lambda-only:1",
            accepted: false,
        },
        Case {
            case: "trusts-edge-only",
            stage: Stage::Role(allow(&format!("\"{EDGE_LAMBDA}\""), "sts:AssumeRole")),
            arn: "arn:aws:lambda:us-east-1:000000000000:function:edge-edge-only:1",
            accepted: false,
        },
        Case {
            case: "trusts-both-in-one-statement",
            stage: Stage::Role(allow(
                &format!("[\"{LAMBDA}\",\"{EDGE_LAMBDA}\"]"),
                "sts:AssumeRole",
            )),
            arn: "arn:aws:lambda:us-east-1:000000000000:function:edge-both-listed:1",
            accepted: true,
        },
        Case {
            case: "trusts-both-over-two-statements",
            stage: Stage::Role(statements(&[("Allow", LAMBDA), ("Allow", EDGE_LAMBDA)])),
            arn: "arn:aws:lambda:us-east-1:000000000000:function:edge-both-split:1",
            accepted: true,
        },
        Case {
            // The action a trust statement names may be a wildcard IAM admits in the
            // assume's place.
            case: "trusts-both-under-an-action-wildcard",
            stage: Stage::Role(allow(&format!("[\"{LAMBDA}\",\"{EDGE_LAMBDA}\"]"), "sts:*")),
            arn: "arn:aws:lambda:us-east-1:000000000000:function:edge-both-wildcard:1",
            accepted: true,
        },
        Case {
            // An explicit deny wins over the grant beside it, so the edge cannot assume the
            // role at all.
            case: "denies-the-edge-beside-the-grant",
            stage: Stage::Role(statements(&[
                ("Allow", LAMBDA),
                ("Allow", EDGE_LAMBDA),
                ("Deny", EDGE_LAMBDA),
            ])),
            arn: "arn:aws:lambda:us-east-1:000000000000:function:edge-denied:1",
            accepted: false,
        },
        Case {
            // A document that parses but states no grant is a policy that grants nothing.
            case: "grants-nothing",
            stage: Stage::Role("{\"Version\":\"2012-10-17\",\"Statement\":[]}".to_string()),
            arn: "arn:aws:lambda:us-east-1:000000000000:function:edge-no-statement:1",
            accepted: false,
        },
        Case {
            case: "trusts-another-service",
            stage: Stage::Role(allow("\"states.amazonaws.com\"", "sts:AssumeRole")),
            arn: "arn:aws:lambda:us-east-1:000000000000:function:edge-other-service:1",
            accepted: false,
        },
        Case {
            // Nothing is known about a role this account's IAM holds no record of, so
            // nothing is refused.
            case: "execution-role-not-held-by-iam",
            stage: Stage::RoleNotHeld,
            arn: "arn:aws:lambda:us-east-1:000000000000:function:edge-unheld-role:1",
            accepted: true,
        },
        Case {
            // Nor about a function Lambda holds nothing under: there is no execution role to
            // resolve at all.
            case: "function-not-held-by-lambda",
            stage: Stage::Nothing,
            arn: "arn:aws:lambda:us-east-1:000000000000:function:edge-absent:1",
            accepted: true,
        },
    ]
}

/// The function name a version ARN names: the segment between `function:` and the version
/// qualifier.
fn function_name_of(arn: &str) -> &str {
    let (function, _version) = arn
        .rsplit_once(':')
        .unwrap_or_else(|| panic!("{arn} is a version ARN"));
    function
        .rsplit_once(':')
        .unwrap_or_else(|| panic!("{arn} names a function"))
        .1
}

/// The role name this suite holds a row's execution role under.
fn role_name_of(arn: &str) -> String {
    format!("{}-role", function_name_of(arn))
}

/// Stage a row's function, its version and its execution role, and answer the role ARN the
/// refusal of that row has to name (`None` for a row no refusal is grounded in).
async fn stage(emulator: &Emulator, case: &Case) -> Option<String> {
    // The literals above carry this emulator's own account and the Region a Lambda@Edge
    // function lives in, so a changed account id fails the suite rather than silently
    // staging a function under another one.
    let prefix = format!("arn:aws:lambda:{REGION}:{ACCOUNT_ID}:function:");
    assert!(
        case.arn.starts_with(&prefix),
        "case {}: the row's ARN must name this account's Region and account ({prefix}), got {}",
        case.case,
        case.arn
    );
    let name = function_name_of(case.arn);
    match &case.stage {
        Stage::Role(trust) => {
            let role_name = role_name_of(case.arn);
            let role_arn = emulator
                .create_role(
                    case.case,
                    &role_name,
                    &statements(&[("Allow", LAMBDA), ("Allow", EDGE_LAMBDA)]),
                )
                .await;
            let version_arn = emulator.publish_function(case.case, name, &role_arn).await;
            assert_eq!(
                version_arn, case.arn,
                "case {}: the published version must be the one the row associates",
                case.case
            );
            emulator.update_trust(case.case, &role_name, trust).await;
            Some(role_arn)
        }
        Stage::RoleNotHeld => {
            let role_arn = format!("arn:aws:iam::{ACCOUNT_ID}:role/{}", role_name_of(case.arn));
            let version_arn = emulator.publish_function(case.case, name, &role_arn).await;
            assert_eq!(
                version_arn, case.arn,
                "case {}: the published version must be the one the row associates",
                case.case
            );
            None
        }
        Stage::Nothing => None,
    }
}

/// The `(LambdaFunctionARN, EventType)` pairs a response reports, wherever it carries an
/// association.
fn reported_associations(answer: &Answer) -> Vec<(String, String)> {
    fn walk(node: &crate::distribution_lifecycle::Node, out: &mut Vec<(String, String)>) {
        for association in node.children_named("LambdaFunctionAssociation") {
            out.push((
                association
                    .text_at("LambdaFunctionARN")
                    .unwrap_or_default()
                    .to_string(),
                association.text_at("EventType").unwrap_or_default().to_string(),
            ));
        }
        for child in &node.children {
            walk(child, out);
        }
    }
    let mut out = Vec::new();
    walk(answer.root(), &mut out);
    out
}

#[tokio::test]
async fn an_edge_association_is_stored_only_where_its_execution_role_admits_the_edge() {
    let emulator = Emulator::new();
    let router = &emulator.router;

    for (index, case) in cases().into_iter().enumerate() {
        let role_arn = stage(&emulator, &case).await;
        for behavior in [Behavior::Default, Behavior::Ordered] {
            let association = Association {
                arn: case.arn,
                event_type: "viewer-request",
            };
            for (path, answer) in [
                (
                    "CreateDistribution",
                    create_distribution(
                        router,
                        &config_xml(
                            &format!("ref-{index}-{behavior:?}-plain"),
                            behavior,
                            &[association],
                        ),
                    )
                    .await,
                ),
                (
                    "CreateDistributionWithTags",
                    create_distribution_with_tags(
                        router,
                        &config_xml(
                            &format!("ref-{index}-{behavior:?}-tagged"),
                            behavior,
                            &[association],
                        ),
                    )
                    .await,
                ),
            ] {
                if case.accepted {
                    assert_eq!(
                        answer.status, 201,
                        "case {} ({path}, {behavior:?}): expected HTTP 201, got {} with {}",
                        case.case, answer.status, answer.raw
                    );
                    let reported = reported_associations(&answer);
                    assert_eq!(
                        reported,
                        vec![(case.arn.to_string(), "viewer-request".to_string())],
                        "case {} ({path}, {behavior:?}): expected the created distribution \
                         to carry the submitted association, got {reported:?}",
                        case.case
                    );
                    continue;
                }
                assert_eq!(
                    answer.status, 400,
                    "case {} ({path}, {behavior:?}): expected HTTP 400, got {} with {}",
                    case.case, answer.status, answer.raw
                );
                assert_eq!(
                    answer.error_code(),
                    "InvalidLambdaFunctionAssociation",
                    "case {} ({path}, {behavior:?}): expected \
                     InvalidLambdaFunctionAssociation, got {:?} with {}",
                    case.case,
                    answer.error_code(),
                    answer.raw
                );
                let message = answer.root().find_text("Message").unwrap_or_default();
                let role_arn = role_arn
                    .as_deref()
                    .unwrap_or_else(|| panic!("case {}: a refused row stages a role", case.case));
                assert_eq!(
                    message,
                    refusal_message(role_arn),
                    "case {} ({path}, {behavior:?}): expected the refusal to state the trust \
                     requirement and name the resolved execution role, got {message:?}",
                    case.case
                );
            }
        }
    }

    // Nothing a refused create named exists: no stored distribution carries an association a
    // refused row submitted. One page carries every accepted row (`MaxItems` admits far
    // more).
    let listed = list_distributions(router, &format!("?MaxItems={MAX_ITEMS}")).await;
    assert_eq!(
        listed.status, 200,
        "case listing: expected HTTP 200, got {} with {}",
        listed.status, listed.raw
    );
    let stored = reported_arns(&listed);
    for case in cases().iter().filter(|case| !case.accepted) {
        assert!(
            !stored.iter().any(|arn| arn == case.arn),
            "case listing: no distribution may carry the refused association {:?} of case {}, \
             got {stored:?}",
            case.arn,
            case.case
        );
    }
}

/// The control the refusal has to leave open: the SAME version ARN is accepted once the role
/// behind it trusts both principals, and the stored configuration echoes the association.
#[tokio::test]
async fn repairing_the_role_lets_the_same_version_be_associated() {
    let emulator = Emulator::new();
    let router = &emulator.router;

    let version_arn = "arn:aws:lambda:us-east-1:000000000000:function:edge-repaired:1";
    let role_name = role_name_of(version_arn);
    let role_arn = emulator
        .create_role(
            "setup",
            &role_name,
            &allow(&format!("\"{LAMBDA}\""), "sts:AssumeRole"),
        )
        .await;
    let published = emulator
        .publish_function("setup", function_name_of(version_arn), &role_arn)
        .await;
    assert_eq!(
        published, version_arn,
        "case setup: expected the published version {version_arn}, got {published}"
    );

    let association = Association {
        arn: version_arn,
        event_type: "origin-request",
    };
    let refused = create_distribution(
        router,
        &config_xml("ref-repaired", Behavior::Default, &[association]),
    )
    .await;
    assert_eq!(
        refused.status, 400,
        "case before-repair: expected HTTP 400, got {} with {}",
        refused.status, refused.raw
    );
    let message = refused.root().find_text("Message").unwrap_or_default();
    assert_eq!(
        message,
        refusal_message(&role_arn),
        "case before-repair: expected the refusal to name {role_arn}, got {message:?}"
    );

    emulator
        .update_trust(
            "repair",
            &role_name,
            &statements(&[("Allow", LAMBDA), ("Allow", EDGE_LAMBDA)]),
        )
        .await;

    let accepted = create_distribution(
        router,
        &config_xml("ref-repaired", Behavior::Default, &[association]),
    )
    .await;
    assert_eq!(
        accepted.status, 201,
        "case after-repair: expected HTTP 201, got {} with {}",
        accepted.status, accepted.raw
    );
    let id = accepted
        .root()
        .text_at("Id")
        .unwrap_or_default()
        .to_string();

    let read = get_distribution_config(router, &id).await;
    assert_eq!(
        read.status, 200,
        "case after-repair-read-back: expected HTTP 200, got {} with {}",
        read.status, read.raw
    );
    let reported = reported_associations(&read);
    assert_eq!(
        reported,
        vec![(version_arn.to_string(), "origin-request".to_string())],
        "case after-repair-read-back: expected the stored configuration to echo the \
         association, got {reported:?}"
    );
}

/// The same rule on the UPDATE path: a distribution cannot be reconfigured onto an
/// association whose execution role withholds the edge's assume, the refused update leaves
/// the stored configuration and version alone, and moving onto a version whose role admits
/// the edge is accepted.
#[tokio::test]
async fn an_update_cannot_move_an_association_onto_a_role_the_edge_cannot_assume() {
    let emulator = Emulator::new();
    let router = &emulator.router;

    let trusted_arn = "arn:aws:lambda:us-east-1:000000000000:function:edge-update-trusted:1";
    let trusted_role = emulator
        .create_role(
            "setup-trusted",
            &role_name_of(trusted_arn),
            &statements(&[("Allow", LAMBDA), ("Allow", EDGE_LAMBDA)]),
        )
        .await;
    emulator
        .publish_function(
            "setup-trusted",
            function_name_of(trusted_arn),
            &trusted_role,
        )
        .await;

    let untrusted_arn = "arn:aws:lambda:us-east-1:000000000000:function:edge-update-untrusted:1";
    let untrusted_role = emulator
        .create_role(
            "setup-untrusted",
            &role_name_of(untrusted_arn),
            &allow(&format!("\"{LAMBDA}\""), "sts:AssumeRole"),
        )
        .await;
    emulator
        .publish_function(
            "setup-untrusted",
            function_name_of(untrusted_arn),
            &untrusted_role,
        )
        .await;

    let created = create_distribution(
        router,
        &config_xml(
            "ref-update",
            Behavior::Default,
            &[Association {
                arn: trusted_arn,
                event_type: "viewer-request",
            }],
        ),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "case update-setup: expected HTTP 201, got {} with {}",
        created.status, created.raw
    );
    let id = created.root().text_at("Id").unwrap_or_default().to_string();
    let etag = created.header("etag").to_string();

    let refused = update_distribution(
        router,
        &id,
        &config_xml(
            "ref-update",
            Behavior::Default,
            &[Association {
                arn: untrusted_arn,
                event_type: "viewer-request",
            }],
        ),
        Some(&etag),
    )
    .await;
    assert_eq!(
        refused.status, 400,
        "case update-to-untrusted: expected HTTP 400, got {} with {}",
        refused.status, refused.raw
    );
    assert_eq!(
        refused.error_code(),
        "InvalidLambdaFunctionAssociation",
        "case update-to-untrusted: expected InvalidLambdaFunctionAssociation, got {:?} with {}",
        refused.error_code(),
        refused.raw
    );
    let message = refused.root().find_text("Message").unwrap_or_default();
    assert_eq!(
        message,
        refusal_message(&untrusted_role),
        "case update-to-untrusted: expected the refusal to name {untrusted_role}, got \
         {message:?}"
    );

    let read = get_distribution_config(router, &id).await;
    let reported = reported_associations(&read);
    assert_eq!(
        reported,
        vec![(trusted_arn.to_string(), "viewer-request".to_string())],
        "case update-to-untrusted: a refused update must leave the stored association in \
         place, got {reported:?}"
    );
    assert_eq!(
        read.header("etag"),
        etag,
        "case update-to-untrusted: a refused update mints no new version, got {:?}",
        read.header("etag")
    );

    // The same update, once the role behind the association admits the edge, is the
    // reconfiguration CloudFront accepts.
    emulator
        .update_trust(
            "repair",
            &role_name_of(untrusted_arn),
            &statements(&[("Allow", LAMBDA), ("Allow", EDGE_LAMBDA)]),
        )
        .await;
    let accepted = update_distribution(
        router,
        &id,
        &config_xml(
            "ref-update",
            Behavior::Default,
            &[Association {
                arn: untrusted_arn,
                event_type: "viewer-request",
            }],
        ),
        Some(&etag),
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "case update-after-repair: expected HTTP 200, got {} with {}",
        accepted.status, accepted.raw
    );
    let reported = reported_associations(&accepted);
    assert_eq!(
        reported,
        vec![(untrusted_arn.to_string(), "viewer-request".to_string())],
        "case update-after-repair: expected the association to name {untrusted_arn}, got \
         {reported:?}"
    );
}
