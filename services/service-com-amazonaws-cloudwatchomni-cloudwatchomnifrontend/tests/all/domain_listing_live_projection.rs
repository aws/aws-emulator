// SPDX-License-Identifier: CC0-1.0

//! `ListDomains` reports the domain records as they stand when the call is answered.
//!
//! `DomainSummary` is documented "Summary of a domain. Call GetDomain for the full
//! domain", and the operation is `@readonly`, so the rule is: every member of a summary is
//! read from the persisted domain record at the time of the listing — a patch that moves a
//! member of the record shows in the next listing, a member no patch moved keeps the value
//! it held, and the summary never differs from what the full reader reports for the same
//! domain.
//!
//! `tests/all/domain_listing.rs` covers the listing of a domain as it was created, its
//! paging and its refusals. This suite drives one record THROUGH a sequence of patches and
//! asserts the projection after each of them, because a listing answered from anything but
//! the live record — a value remembered at create time, or one rebuilt from the request —
//! reports a domain that no longer exists.
//!
//! Rows differ along what a patch moves: the name alone, the provider set together with
//! the Identity Center instance behind it, the instance alone, NOTHING at all (a patch
//! asking for the values the domain already holds), and the Identity Center configuration
//! cleared. A record replaced by a delete and a fresh create is covered too, so no summary
//! can be a projection of a record that is gone.

use std::sync::Arc;

use aws_smithy_types::DateTime;
use smithy_server_runtime::{
    EmbeddedEmulator, MultiServiceRouter, Operation, OperationContext, Service,
};

use service_com_amazonaws_cloudwatchomni_cloudwatchomnifrontend as omni;

use omni::ops::create_domain::CreateDomain;
use omni::ops::delete_domain::DeleteDomain;
use omni::ops::get_domain::GetDomain;
use omni::ops::list_domains::ListDomains;
use omni::ops::types::{
    CreateDomainInput, DeleteDomainInput, Domain, DomainSummary, GetDomainInput,
    IdentityCenterConfiguration, IdentityProviderConfiguration, ListDomainsInput,
    UpdateDomainInput,
};
use omni::ops::update_domain::UpdateDomain;

/// The Region every call below is signed for, which is the Region the record carries and
/// the listing is expected to report back.
const REGION: &str = "us-west-2";

/// Two Identity Center instance ARNs satisfying `IdentityCenterInstanceArn`'s modelled
/// `^arn:aws[a-z-]*:sso:::instance/[a-zA-Z0-9-]+$`, so a patch can move a domain from one
/// instance to another.
const IDC_INSTANCE_ONE: &str = "arn:aws:sso:::instance/ssoins-1111111111111111";
const IDC_INSTANCE_TWO: &str = "arn:aws:sso:::instance/ssoins-2222222222222222";

/// What a patch asks of the domain's identity-provider configuration: leave it alone, carry
/// it with no Identity Center block at all, or name an instance.
#[derive(Debug, Clone, Copy)]
enum Configuration {
    Absent,
    Empty,
    Instance(&'static str),
}

impl Configuration {
    fn build(self) -> Option<IdentityProviderConfiguration> {
        match self {
            Configuration::Absent => None,
            Configuration::Empty => Some(IdentityProviderConfiguration {
                identity_center_configuration: None,
            }),
            Configuration::Instance(arn) => Some(IdentityProviderConfiguration {
                identity_center_configuration: Some(IdentityCenterConfiguration {
                    identity_center_instance_arn: Some(arn.to_string()),
                }),
            }),
        }
    }
}

/// One patch the sequence applies, and what the listing must report once it is served.
#[derive(Debug)]
struct Row {
    case: &'static str,
    /// The members the patch carries; a `None` leaves that member of the record alone.
    patch_name: Option<&'static str>,
    patch_providers: Option<&'static [&'static str]>,
    patch_configuration: Configuration,
    /// The name and the Identity Center instance the summary must report afterwards.
    expected_name: &'static str,
    expected_instance: Option<&'static str>,
    /// Whether the patch moves a member of the record at all: a patch that moves nothing
    /// must leave `updatedAt` exactly where it was, because that member is "The timestamp
    /// when the domain was last updated".
    moves_a_member: bool,
}

/// The patches applied in order to the one domain an account holds, starting from an
/// IAM-only domain named `omni-projected`.
const ROWS: &[Row] = &[
    Row {
        case: "a rename, carrying nothing else",
        patch_name: Some("omni-projected-renamed"),
        patch_providers: None,
        patch_configuration: Configuration::Absent,
        expected_name: "omni-projected-renamed",
        expected_instance: None,
        moves_a_member: true,
    },
    Row {
        case: "adding IDC beside IAM with the instance behind it",
        patch_name: None,
        patch_providers: Some(&["IAM", "IDC"]),
        patch_configuration: Configuration::Instance(IDC_INSTANCE_ONE),
        expected_name: "omni-projected-renamed",
        expected_instance: Some(IDC_INSTANCE_ONE),
        moves_a_member: true,
    },
    Row {
        case: "moving the domain to another Identity Center instance",
        patch_name: None,
        patch_providers: None,
        patch_configuration: Configuration::Instance(IDC_INSTANCE_TWO),
        expected_name: "omni-projected-renamed",
        expected_instance: Some(IDC_INSTANCE_TWO),
        moves_a_member: true,
    },
    Row {
        case: "a patch asking for the values the domain already holds",
        patch_name: Some("omni-projected-renamed"),
        patch_providers: Some(&["IAM", "IDC"]),
        patch_configuration: Configuration::Instance(IDC_INSTANCE_TWO),
        expected_name: "omni-projected-renamed",
        expected_instance: Some(IDC_INSTANCE_TWO),
        moves_a_member: false,
    },
    Row {
        case: "clearing the Identity Center configuration, leaving IAM",
        patch_name: Some("omni-projected-iam"),
        patch_providers: Some(&["IAM"]),
        patch_configuration: Configuration::Empty,
        expected_name: "omni-projected-iam",
        expected_instance: None,
        moves_a_member: true,
    },
];

/// The members of a `DomainSummary`, as the model lists them
/// (`smithy_query(query="shape", shape="DomainSummary")`): `domainId`, `createdAt`,
/// `updatedAt` and `status` are `@required`; `domainArn`, `name`,
/// `identityCenterInstanceArn` and `region` are optional.
#[derive(Debug, PartialEq)]
struct Projection {
    domain_id: String,
    domain_arn: Option<String>,
    name: Option<String>,
    identity_center_instance_arn: Option<String>,
    region: Option<String>,
    created_at: DateTime,
    updated_at: DateTime,
    status: String,
}

/// The summary the listing reported, read member by member.
fn of_summary(summary: &DomainSummary) -> Projection {
    Projection {
        domain_id: summary.domain_id.clone(),
        domain_arn: summary.domain_arn.clone(),
        name: summary.name.clone(),
        identity_center_instance_arn: summary.identity_center_instance_arn.clone(),
        region: summary.region.clone(),
        created_at: summary.created_at,
        updated_at: summary.updated_at,
        status: summary.status.clone(),
    }
}

/// The same members read off the full record the reader answers with — the values the
/// summary has to carry, since a summary reports a domain rather than describing one.
fn of_record(domain: &Domain) -> Projection {
    Projection {
        domain_id: domain.domain_id.clone(),
        domain_arn: Some(domain.domain_arn.clone()),
        name: domain.name.clone(),
        identity_center_instance_arn: domain
            .identity_provider_configuration
            .as_ref()
            .and_then(|configuration| configuration.identity_center_configuration.as_ref())
            .and_then(|center| center.identity_center_instance_arn.clone()),
        region: Some(domain.region.clone()),
        created_at: domain.created_at,
        updated_at: domain.updated_at,
        status: domain.status.clone(),
    }
}

/// One emulator: one shared engine, reached by every operation below.
struct Omni {
    engine: Arc<engine::InMemoryEngine>,
    service: omni::EmulatedService<engine::InMemoryEngine>,
    router: MultiServiceRouter,
    parts: http::request::Parts,
}

impl Omni {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(omni::EmulatedService::new(Arc::clone(&engine)));
        let (parts, ()) = http::Request::builder()
            .method("POST")
            .uri("/service/CloudWatchOmniFrontend/operation/Unused")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEMULATOR/20260101/{REGION}/cloudwatch/\
                     aws4_request, SignedHeaders=host, Signature=emulator"
                ),
            )
            .body(())
            .expect("a well-formed request")
            .into_parts();
        Self {
            service: omni::EmulatedService::new(Arc::clone(&engine)),
            engine,
            router,
            parts,
        }
    }

    fn ctx(&self) -> OperationContext<'_> {
        OperationContext::new(
            &self.parts,
            self.service.definition().identity(),
            EmbeddedEmulator::from_router(self.router.clone()),
        )
    }

    /// The account's domain, created with the providers the caller asks for.
    async fn create_domain(&self, case: &str, name: &str, providers: &[&str]) -> Domain {
        CreateDomain::new(Arc::clone(&self.engine))
            .handle(
                CreateDomainInput {
                    name: name.to_string(),
                    identity_providers: providers.iter().map(|p| (*p).to_string()).collect(),
                    client_token: Some(format!("token-{name}")),
                    ..Default::default()
                },
                &self.ctx(),
            )
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: CreateDomain failed: {error:?}"))
            .domain
    }

    /// The record `row` leaves the domain holding.
    async fn update_domain(&self, row: &Row, domain_id: &str) -> Domain {
        UpdateDomain::new(Arc::clone(&self.engine))
            .handle(
                UpdateDomainInput {
                    domain_id: domain_id.to_string(),
                    name: row.patch_name.map(str::to_string),
                    identity_providers: row.patch_providers.map(|providers| {
                        providers
                            .iter()
                            .map(|provider| (*provider).to_string())
                            .collect()
                    }),
                    identity_provider_configuration: row.patch_configuration.build(),
                },
                &self.ctx(),
            )
            .await
            .unwrap_or_else(|error| {
                panic!("case {:?}: UpdateDomain failed: {error:?}", row.case)
            })
            .domain
    }

    /// The full record, through the reader a caller resolves a domain with.
    async fn read_domain(&self, case: &str, domain_id: &str) -> Domain {
        GetDomain::new(Arc::clone(&self.engine))
            .handle(
                GetDomainInput {
                    domain_id: domain_id.to_string(),
                },
                &self.ctx(),
            )
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: GetDomain failed: {error:?}"))
            .domain
    }

    async fn delete_domain(&self, case: &str, domain_id: &str) {
        DeleteDomain::new(Arc::clone(&self.engine))
            .handle(
                DeleteDomainInput {
                    domain_id: domain_id.to_string(),
                },
                &self.ctx(),
            )
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: DeleteDomain failed: {error:?}"));
    }

    /// The one summary the account's listing reports, required to be the domain `domain_id`
    /// names.
    async fn listed(&self, case: &str, domain_id: &str) -> DomainSummary {
        let page = ListDomains::new(Arc::clone(&self.engine))
            .handle(ListDomainsInput::default(), &self.ctx())
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: ListDomains failed: {error:?}"));
        assert!(
            page.next_token.is_none(),
            "case {case:?}: nothing remains beyond the account's one domain, got token {:?}",
            page.next_token
        );
        let ids: Vec<&str> = page
            .items
            .iter()
            .map(|item| item.domain_id.as_str())
            .collect();
        assert_eq!(
            ids,
            vec![domain_id],
            "case {case:?}: expected the listing to report the account's domain, got {page:?}"
        );
        page.items
            .into_iter()
            .next()
            .expect("the listing reports one domain")
    }
}

#[tokio::test]
async fn every_summary_member_is_the_record_s_after_each_patch() {
    let omni = Omni::new();
    let created = omni
        .create_domain("the domain as created", "omni-projected", &["IAM"])
        .await;
    let domain_id = created.domain_id.clone();

    // The identity of the record: these members are in no patch's input, so no row may
    // move them.
    let identity = (
        created.domain_id.clone(),
        created.domain_arn.clone(),
        created.region.clone(),
        created.status.clone(),
        created.created_at,
    );

    let summary = omni.listed("the domain as created", &domain_id).await;
    let record = omni.read_domain("the domain as created", &domain_id).await;
    assert_eq!(
        of_summary(&summary),
        of_record(&record),
        "case {:?}: the summary is not the record's projection",
        "the domain as created"
    );
    let mut previous_updated_at = record.updated_at;

    for row in ROWS {
        let case = row.case;
        let patched = omni.update_domain(row, &domain_id).await;
        let record = omni.read_domain(case, &domain_id).await;
        assert_eq!(
            of_record(&patched),
            of_record(&record),
            "case {case:?}: the reader and the patch report different records"
        );

        let summary = omni.listed(case, &domain_id).await;
        assert_eq!(
            of_summary(&summary),
            of_record(&record),
            "case {case:?}: the summary is not the live record's projection"
        );
        assert_eq!(
            (summary.name.as_deref(), summary.identity_center_instance_arn.as_deref()),
            (Some(row.expected_name), row.expected_instance),
            "case {case:?}: expected the patched name and instance, got {summary:?}"
        );
        assert_eq!(
            (
                summary.domain_id.clone(),
                summary.domain_arn.clone().unwrap_or_default(),
                summary.region.clone().unwrap_or_default(),
                summary.status.clone(),
                summary.created_at,
            ),
            identity,
            "case {case:?}: a patch moved a member no patch carries, got {summary:?}"
        );
        // `updatedAt` is "The timestamp when the domain was last updated": a patch that
        // moves nothing leaves it exactly where it was, and one that moves a member never
        // reports an instant before the one the record already held.
        if row.moves_a_member {
            assert!(
                summary.updated_at >= previous_updated_at,
                "case {case:?}: updatedAt went backwards, from {previous_updated_at:?} to \
                 {:?}",
                summary.updated_at
            );
        } else {
            assert_eq!(
                summary.updated_at, previous_updated_at,
                "case {case:?}: a patch that moved nothing moved updatedAt to {:?}",
                summary.updated_at
            );
        }
        previous_updated_at = summary.updated_at;
    }
}

#[tokio::test]
async fn the_summary_follows_the_record_a_delete_and_a_fresh_create_leave() {
    let omni = Omni::new();
    let first = omni
        .create_domain("the first record", "omni-projected-first", &["IAM"])
        .await;
    let listed_first = omni.listed("the first record", &first.domain_id).await;
    assert_eq!(
        of_summary(&listed_first),
        of_record(&omni.read_domain("the first record", &first.domain_id).await),
        "case {:?}: the summary is not the record's projection",
        "the first record"
    );

    omni.delete_domain("the first record", &first.domain_id).await;
    let second = omni
        .create_domain("the second record", "omni-projected-second", &["IAM"])
        .await;
    assert_ne!(
        second.domain_id, first.domain_id,
        "a fresh create mints a domain of its own, got {:?} twice",
        second.domain_id
    );

    let listed_second = omni.listed("the second record", &second.domain_id).await;
    assert_eq!(
        of_summary(&listed_second),
        of_record(&omni.read_domain("the second record", &second.domain_id).await),
        "case {:?}: the summary is not the record's projection",
        "the second record"
    );
    assert_ne!(
        listed_second.domain_arn, listed_first.domain_arn,
        "the listing reports the replaced record's ARN {:?}",
        listed_second.domain_arn
    );
}
