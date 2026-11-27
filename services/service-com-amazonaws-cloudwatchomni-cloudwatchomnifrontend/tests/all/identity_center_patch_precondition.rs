// SPDX-License-Identifier: CC0-1.0

//! What a client reads off the rpcv2Cbor wire when it patches a domain's identity
//! federation, and what the store holds afterwards.
//!
//! The RULE this suite asserts, one level above any single request: a patch that would
//! leave a domain listing the `IDC` identity provider with no Identity Center instance
//! behind it is REFUSED — HTTP 400 `ValidationException`, one constant sentence — before
//! anything is written, so the domain keeps every member it held and no Identity Center
//! application is minted for it. The rule is the model's own:
//! `IdentityProviderConfiguration.identityCenterConfiguration` is documented "Identity
//! Center configuration. Required when identityProviders includes IDC", and
//! `Domain.identityCenterApplicationArn` is documented "The ARN of the Identity Center
//! application. Absent for IAM-only domains"
//! (`smithy_query(query="shape", shape="IdentityProviderConfiguration,Domain")`).
//! `UpdateDomain` declares `ValidationException` with `@httpError 400`
//! (`smithy_query(query="operation", operation="UpdateDomain")`); the model fixes no
//! message text, so the wording is recorded behaviour and is written out below.
//!
//! The rule is decided from the state the patch would LEAVE the domain in rather than from
//! the members one request happened to carry, so the rows span that class:
//!
//! * the resulting provider list's form — `IDC` alone, `IDC` beside `IAM` in either order
//!   (`IdentityProviderList` is `@length(1..2)`, `@uniqueItems`, over `IdentityProvider`
//!   [IAM, IDC]);
//! * how the instance comes to be missing — the request names no
//!   `identityProviderConfiguration` at all, names an empty one, names one whose
//!   `identityCenterConfiguration` carries no instance ARN, or takes away the instance a
//!   domain already federated through;
//! * whether the patch also renames the domain, which is a member of the same request and
//!   must not be applied either.
//!
//! [`a_patch_that_leaves_the_domain_federable_is_served`] is the control: the same patches
//! with an instance behind them — and the patches that touch no federation at all — are
//! served, and the Identity Center application the domain then reports is minted under the
//! instance the request named, so the refusal above is the precondition rather than a
//! blanket refusal of the member.
//!
//! Every call goes through the router over rpcv2Cbor, so routing, deserialization, the
//! handler, the store and response serialization all run: the state checks after each
//! refusal are read back through `GetDomain` and `ListDomains`, the way a client reads
//! them.
//!
//! The wire harness, the row table and the refusal's wording below are `pub(crate)` because
//! [`crate::identity_center_patch_refusal_ordering`] asserts the same rule over the same
//! rows from the calls that FOLLOW a refusal, and drives them through this harness rather
//! than a second copy of it.

use std::sync::Arc;

use aws_smithy_schema::codec::{Codec, FinishSerializer};
use aws_smithy_schema::serde::{SerializableStruct, ShapeSerializer};
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::{CborCodec, DeserializableStruct};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cloudwatchomni_cloudwatchomnifrontend as omni;

use omni::ops::types::{
    CreateDomainInput, CreateDomainOutput, Domain, GetDomainInput, GetDomainOutput,
    IdentityCenterConfiguration, IdentityProviderConfiguration, ListDomainsInput, ListDomainsOutput,
    UpdateDomainInput, UpdateDomainOutput, CREATE_DOMAIN_INPUT_SCHEMA, GET_DOMAIN_INPUT_SCHEMA,
    LIST_DOMAINS_INPUT_SCHEMA, UPDATE_DOMAIN_INPUT_SCHEMA,
};

/// The envelope reader the suites that read a refusal off the wire share.
use super::validation_message_wording::error_envelope;

/// The error `UpdateDomain` models for a request it cannot apply, and its status.
pub(crate) const VALIDATION_EXCEPTION: &str = "ValidationException";
pub(crate) const VALIDATION_STATUS: u16 = 400;

/// The whole sentence a patch that would leave a domain federating through Identity Center
/// with no instance behind it earns, written out here so a change to the wording fails this
/// suite rather than following the implementation.
///
/// Wholly fixed text: it names the member whose value is missing and the provider that
/// required it, and carries nothing of the request — no `domainId`, no name, no provider
/// list.
pub(crate) const PRECONDITION_MESSAGE: &str =
    "identityCenterInstanceArn is required when authorizationProviders includes IDC";

/// The region every call below is signed for.
const REGION: &str = "us-west-2";

/// An Identity Center instance ARN satisfying `IdentityCenterInstanceArn`'s modelled
/// `^arn:aws[a-z-]*:sso:::instance/[a-zA-Z0-9-]+$`.
pub(crate) const INSTANCE_ARN: &str = "arn:aws:sso:::instance/ssoins-1234567890abcdef";

/// A second instance ARN, for the patch that moves a domain from one instance to another.
const OTHER_INSTANCE_ARN: &str = "arn:aws:sso:::instance/ssoins-fedcba0987654321";

/// The `identityProviderConfiguration` a request carries: naming `instance`, or naming an
/// `identityCenterConfiguration` with no instance ARN, or empty.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Configuration {
    /// No `identityProviderConfiguration` member at all.
    Absent,
    /// The member, present and empty — no `identityCenterConfiguration`.
    Empty,
    /// An `identityCenterConfiguration` that names no instance ARN.
    WithoutInstance,
    /// An `identityCenterConfiguration` naming this instance.
    Instance(&'static str),
}

impl Configuration {
    pub(crate) fn build(self) -> Option<IdentityProviderConfiguration> {
        match self {
            Self::Absent => None,
            Self::Empty => Some(IdentityProviderConfiguration::default()),
            Self::WithoutInstance => Some(IdentityProviderConfiguration {
                identity_center_configuration: Some(IdentityCenterConfiguration::default()),
            }),
            Self::Instance(arn) => Some(IdentityProviderConfiguration {
                identity_center_configuration: Some(IdentityCenterConfiguration {
                    identity_center_instance_arn: Some(arn.to_string()),
                }),
            }),
        }
    }
}

/// One row: the domain it provisions and the patch it then submits.
#[derive(Clone, Debug)]
pub(crate) struct Row {
    /// What the row is about, named in every assertion it makes.
    pub(crate) case: &'static str,
    pub(crate) stored_name: &'static str,
    pub(crate) stored_providers: &'static [&'static str],
    stored_configuration: Configuration,
    pub(crate) patch_name: Option<&'static str>,
    pub(crate) patch_providers: Option<&'static [&'static str]>,
    patch_configuration: Configuration,
}

impl Row {
    /// The provider list the patch would leave the domain with: the one it names, or the one
    /// the domain already holds when it names none.
    pub(crate) fn resulting_providers(&self) -> Vec<String> {
        self.patch_providers
            .unwrap_or(self.stored_providers)
            .iter()
            .map(|provider| (*provider).to_string())
            .collect()
    }

    /// The name the patch would leave the domain with.
    pub(crate) fn resulting_name(&self) -> &'static str {
        self.patch_name.unwrap_or(self.stored_name)
    }
}

/// One emulator: the service mounted on a router over one engine, so a create is
/// observable by the reader that runs next.
pub(crate) struct Wire {
    router: MultiServiceRouter,
}

impl Wire {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(omni::EmulatedService::new(engine));
        Self { router }
    }

    /// Dispatch one rpcv2Cbor request and return its status and body bytes.
    async fn call<I: SerializableStruct>(
        &self,
        operation: &str,
        input_schema: &Schema,
        input: &I,
    ) -> (u16, bytes::Bytes) {
        let codec = CborCodec::default();
        let mut serializer = codec.create_serializer();
        serializer
            .write_struct(input_schema, input)
            .expect("the input shape serializes");
        let request = http::Request::builder()
            .method("POST")
            .uri(format!(
                "/service/CloudWatchOmniFrontend/operation/{operation}"
            ))
            .header("content-type", "application/cbor")
            .header("smithy-protocol", "rpc-v2-cbor")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEMULATOR/20260101/{REGION}/cloudwatch/\
                     aws4_request, SignedHeaders=host, Signature=emulator"
                ),
            )
            .body(bytes::Bytes::from(serializer.finish()))
            .expect("a well-formed rpcv2Cbor request");
        let response = self.router.dispatch(request).await;
        (response.status().as_u16(), response.body().clone())
    }

    /// The account's domain, created over the same wire.
    pub(crate) async fn create_domain(&self, row: &Row) -> Domain {
        let case = row.case;
        let (status, body) = self
            .call(
                "CreateDomain",
                CREATE_DOMAIN_INPUT_SCHEMA,
                &CreateDomainInput {
                    name: row.stored_name.to_string(),
                    identity_providers: row
                        .stored_providers
                        .iter()
                        .map(|provider| (*provider).to_string())
                        .collect(),
                    identity_provider_configuration: row.stored_configuration.build(),
                    client_token: Some(format!("token-{}", row.stored_name)),
                    ..Default::default()
                },
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateDomain answered {status}: {:?}",
            String::from_utf8_lossy(&body)
        );
        decode::<CreateDomainOutput>(case, "CreateDomain", &body).domain
    }

    /// The patch the row submits, as the status and body it answered with.
    pub(crate) async fn update_domain(&self, row: &Row, domain_id: &str) -> (u16, bytes::Bytes) {
        self.update_domain_carrying(row, domain_id, row.patch_configuration)
            .await
    }

    /// The row's patch carrying `configuration` in place of its own — how a refused patch is
    /// resubmitted once it names an Identity Center instance.
    pub(crate) async fn update_domain_carrying(
        &self,
        row: &Row,
        domain_id: &str,
        configuration: Configuration,
    ) -> (u16, bytes::Bytes) {
        self.call(
            "UpdateDomain",
            UPDATE_DOMAIN_INPUT_SCHEMA,
            &UpdateDomainInput {
                domain_id: domain_id.to_string(),
                name: row.patch_name.map(str::to_string),
                identity_providers: row.patch_providers.map(|providers| {
                    providers
                        .iter()
                        .map(|provider| (*provider).to_string())
                        .collect()
                }),
                identity_provider_configuration: configuration.build(),
            },
        )
        .await
    }

    /// The domain the store holds, read back through the operation that reports it.
    pub(crate) async fn get_domain(&self, case: &str, domain_id: &str) -> Domain {
        let (status, body) = self
            .call(
                "GetDomain",
                GET_DOMAIN_INPUT_SCHEMA,
                &GetDomainInput {
                    domain_id: domain_id.to_string(),
                },
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetDomain answered {status}: {:?}",
            String::from_utf8_lossy(&body)
        );
        decode::<GetDomainOutput>(case, "GetDomain", &body).domain
    }

    /// The Identity Center instance the account's listing reports for `domain_id`.
    pub(crate) async fn listed_instance_arn(&self, case: &str, domain_id: &str) -> Option<String> {
        let (status, body) = self
            .call(
                "ListDomains",
                LIST_DOMAINS_INPUT_SCHEMA,
                &ListDomainsInput::default(),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListDomains answered {status}: {:?}",
            String::from_utf8_lossy(&body)
        );
        let page = decode::<ListDomainsOutput>(case, "ListDomains", &body);
        let summary = page
            .items
            .iter()
            .find(|item| item.domain_id == domain_id)
            .unwrap_or_else(|| {
                panic!("case {case:?}: the listing does not report the domain: {page:?}")
            });
        summary.identity_center_instance_arn.clone()
    }
}

/// Decode one success body into the output shape it carries.
pub(crate) fn decode<O: DeserializableStruct>(
    case: &str,
    operation: &str,
    body: &bytes::Bytes,
) -> O {
    let codec = CborCodec::default();
    let mut deserializer = codec.create_deserializer(body);
    O::deserialize(&mut deserializer)
        .unwrap_or_else(|error| panic!("case {case:?}: {operation}'s output: {error}"))
}

/// The instance ARN a domain's identity-provider configuration names.
pub(crate) fn instance_arn(domain: &Domain) -> Option<&str> {
    domain
        .identity_provider_configuration
        .as_ref()
        .and_then(|configuration| configuration.identity_center_configuration.as_ref())
        .and_then(|center| center.identity_center_instance_arn.as_deref())
}

/// The rows whose patch would leave the domain listing IDC with no instance behind it.
pub(crate) fn refused_rows() -> Vec<Row> {
    vec![
        Row {
            case: "switching an IAM-only domain to IDC",
            stored_name: "omni-switch",
            stored_providers: &["IAM"],
            stored_configuration: Configuration::Absent,
            patch_name: None,
            patch_providers: Some(&["IDC"]),
            patch_configuration: Configuration::Absent,
        },
        Row {
            case: "adding IDC beside IAM",
            stored_name: "omni-add-idc",
            stored_providers: &["IAM"],
            stored_configuration: Configuration::Absent,
            patch_name: None,
            patch_providers: Some(&["IAM", "IDC"]),
            patch_configuration: Configuration::Absent,
        },
        Row {
            case: "adding IDC first in the provider list",
            stored_name: "omni-idc-first",
            stored_providers: &["IAM"],
            stored_configuration: Configuration::Absent,
            patch_name: None,
            patch_providers: Some(&["IDC", "IAM"]),
            patch_configuration: Configuration::Absent,
        },
        Row {
            case: "switching to IDC and renaming in the same request",
            stored_name: "omni-switch-rename",
            stored_providers: &["IAM"],
            stored_configuration: Configuration::Absent,
            patch_name: Some("omni-switch-renamed"),
            patch_providers: Some(&["IDC"]),
            patch_configuration: Configuration::Absent,
        },
        Row {
            case: "switching to IDC with an empty identityProviderConfiguration",
            stored_name: "omni-empty-config",
            stored_providers: &["IAM"],
            stored_configuration: Configuration::Absent,
            patch_name: None,
            patch_providers: Some(&["IDC"]),
            patch_configuration: Configuration::Empty,
        },
        Row {
            case: "switching to IDC with an identityCenterConfiguration naming no instance",
            stored_name: "omni-blank-instance",
            stored_providers: &["IAM"],
            stored_configuration: Configuration::Absent,
            patch_name: None,
            patch_providers: Some(&["IDC"]),
            patch_configuration: Configuration::WithoutInstance,
        },
        Row {
            case: "taking the instance away from a domain that keeps listing IDC",
            stored_name: "omni-instance-removed",
            stored_providers: &["IDC"],
            stored_configuration: Configuration::Instance(INSTANCE_ARN),
            patch_name: None,
            patch_providers: None,
            patch_configuration: Configuration::Empty,
        },
        Row {
            case: "renaming while taking the instance away",
            stored_name: "omni-instance-dropped",
            stored_providers: &["IAM", "IDC"],
            stored_configuration: Configuration::Instance(INSTANCE_ARN),
            patch_name: Some("omni-instance-dropped-renamed"),
            patch_providers: None,
            patch_configuration: Configuration::WithoutInstance,
        },
    ]
}

/// The rows whose patch leaves the domain federable, or touches no federation at all.
fn served_rows() -> Vec<Row> {
    vec![
        Row {
            case: "switching to IDC and naming an instance",
            stored_name: "omni-served-switch",
            stored_providers: &["IAM"],
            stored_configuration: Configuration::Absent,
            patch_name: None,
            patch_providers: Some(&["IDC"]),
            patch_configuration: Configuration::Instance(INSTANCE_ARN),
        },
        Row {
            case: "adding IDC to a domain that already holds an instance",
            stored_name: "omni-served-add",
            stored_providers: &["IAM"],
            stored_configuration: Configuration::Instance(INSTANCE_ARN),
            patch_name: Some("omni-served-added"),
            patch_providers: Some(&["IAM", "IDC"]),
            patch_configuration: Configuration::Absent,
        },
        Row {
            case: "moving a domain that lists IDC to another instance",
            stored_name: "omni-served-moved",
            stored_providers: &["IDC"],
            stored_configuration: Configuration::Instance(INSTANCE_ARN),
            patch_name: None,
            patch_providers: None,
            patch_configuration: Configuration::Instance(OTHER_INSTANCE_ARN),
        },
        Row {
            case: "dropping IDC needs no instance",
            stored_name: "omni-served-dropped",
            stored_providers: &["IDC"],
            stored_configuration: Configuration::Instance(INSTANCE_ARN),
            patch_name: None,
            patch_providers: Some(&["IAM"]),
            patch_configuration: Configuration::Absent,
        },
        Row {
            case: "a rename alone touches no federation",
            stored_name: "omni-served-rename",
            stored_providers: &["IAM"],
            stored_configuration: Configuration::Absent,
            patch_name: Some("omni-served-renamed"),
            patch_providers: None,
            patch_configuration: Configuration::Absent,
        },
    ]
}

#[tokio::test]
async fn a_patch_that_would_leave_idc_without_an_instance_is_refused_and_writes_nothing() {
    for row in refused_rows() {
        let case = row.case;
        // An account holds at most ONE domain, so each row patches the domain of an
        // emulator of its own.
        let wire = Wire::new();
        let created = wire.create_domain(&row).await;
        // What the store holds before the patch, read through the reader that reports it,
        // so the comparison afterwards is between two reads of the same record.
        let held = wire.get_domain(case, &created.domain_id).await;
        let before = format!("{held:?}");
        let listed_before = wire.listed_instance_arn(case, &created.domain_id).await;

        let (status, body) = wire.update_domain(&row, &created.domain_id).await;
        let (code, message) = error_envelope(case, &body);
        assert_eq!(
            (status, code.as_str(), message.as_str()),
            (VALIDATION_STATUS, VALIDATION_EXCEPTION, PRECONDITION_MESSAGE),
            "case {case:?}: expected {VALIDATION_STATUS} {VALIDATION_EXCEPTION} \
             {PRECONDITION_MESSAGE:?}, got {status} {code:?}: {message:?}"
        );
        // One constant sentence: nothing of the request reaches it.
        for (what, value) in [
            ("the domain id", created.domain_id.as_str()),
            ("the stored name", row.stored_name),
            ("the requested name", row.patch_name.unwrap_or(row.stored_name)),
            ("the operation name", "UpdateDomain"),
        ] {
            assert!(
                !message.contains(value),
                "case {case:?}: the refusal renders {what} ({value:?}): {message:?}"
            );
        }

        // The refusal wrote nothing: every member the domain held is still reported,
        // including the provider list, the configuration and the absence or presence of
        // the Identity Center application.
        let after = wire.get_domain(case, &created.domain_id).await;
        assert_eq!(
            format!("{after:?}"),
            before,
            "case {case:?}: the refused patch changed the domain"
        );
        let expected_providers: Vec<String> = row
            .stored_providers
            .iter()
            .map(|provider| (*provider).to_string())
            .collect();
        assert_eq!(
            after.identity_providers, expected_providers,
            "case {case:?}: expected the stored providers {expected_providers:?}, got {:?}",
            after.identity_providers
        );
        assert_eq!(
            after.name.as_deref(),
            Some(row.stored_name),
            "case {case:?}: expected the stored name {:?}, got {:?}",
            row.stored_name,
            after.name
        );
        let expected_application = held.identity_center_application_arn.clone();
        assert_eq!(
            after.identity_center_application_arn, expected_application,
            "case {case:?}: expected the Identity Center application to stay \
             {expected_application:?}, got {:?}",
            after.identity_center_application_arn
        );
        let listed_after = wire.listed_instance_arn(case, &created.domain_id).await;
        assert_eq!(
            listed_after, listed_before,
            "case {case:?}: expected the listing to keep reporting {listed_before:?}, got \
             {listed_after:?}"
        );
    }
}

#[tokio::test]
async fn a_patch_that_leaves_the_domain_federable_is_served() {
    for row in served_rows() {
        let case = row.case;
        let wire = Wire::new();
        let created = wire.create_domain(&row).await;

        let (status, body) = wire.update_domain(&row, &created.domain_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the patch must be served, got {status}: {:?}",
            String::from_utf8_lossy(&body)
        );
        let patched = decode::<UpdateDomainOutput>(case, "UpdateDomain", &body).domain;

        // Whether the patched domain federates through Identity Center is read from the
        // provider list it is left with, and the application ARN the model documents as
        // "Absent for IAM-only domains" follows it — minted under the instance the domain
        // is left naming, never out of nothing.
        let lists_idc = patched
            .identity_providers
            .iter()
            .any(|provider| provider == "IDC");
        let left_instance = instance_arn(&patched).map(str::to_string);
        assert!(
            !lists_idc || left_instance.is_some(),
            "case {case:?}: a domain listing IDC must be left with an instance, got \
             providers {:?} and configuration {:?}",
            patched.identity_providers,
            patched.identity_provider_configuration
        );
        match (lists_idc, patched.identity_center_application_arn.as_deref()) {
            (true, Some(application)) => {
                let instance = left_instance
                    .as_deref()
                    .and_then(|arn| arn.split('/').next_back())
                    .unwrap_or_default();
                assert!(
                    application.contains(instance),
                    "case {case:?}: the application {application:?} must name the instance \
                     {instance:?} the domain federates through"
                );
            }
            (true, None) => panic!(
                "case {case:?}: a domain listing IDC must report an Identity Center application"
            ),
            (false, application) => assert_eq!(
                application, None,
                "case {case:?}: an IAM-only domain must report no Identity Center \
                 application, got {application:?}"
            ),
        }

        // The patch persisted: the reader answers from the same store, and the listing
        // reports the instance the domain is left federating through.
        let read_back = wire.get_domain(case, &created.domain_id).await;
        assert_eq!(
            format!("{read_back:?}"),
            format!("{patched:?}"),
            "case {case:?}: GetDomain must report the patched domain"
        );
        let listed = wire.listed_instance_arn(case, &created.domain_id).await;
        assert_eq!(
            listed, left_instance,
            "case {case:?}: expected the listing to report {left_instance:?}, got {listed:?}"
        );
    }
}
