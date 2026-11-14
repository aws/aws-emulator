// SPDX-License-Identifier: CC0-1.0

//! What a connector's creation LANDS in, decided by the resources its create referenced.
//!
//! The RULE these cases assert, over a table rather than one recorded request: a create
//! references resources other services own — the private certificate authority Amazon Web
//! Services Private CA holds, and the VPC endpoint Amazon EC2 holds when the request restricts
//! the connector to one — so each reference is resolved against the service that OWNS it, and
//! a reference that service reports it cannot serve makes the creation land in `FAILED`
//! carrying the `ConnectorStatusReason` for that cause instead of in `ACTIVE`.
//!
//! Why the outcome is reported on the connector rather than refused at the call:
//! `CreateConnector` is asynchronous (its modelled success is HTTP 202 and `ConnectorStatus`
//! carries `CREATING`), and `StatusReason` is documented as "Information about why connector
//! creation failed, if status is `FAILED`" (smithy_query shape=Connector member=StatusReason)
//! over an enum whose values name both kinds of reference (smithy_query
//! shape=ConnectorStatusReason) — values only a connector that was created and then failed can
//! report. The one reference resolved BEFORE a record exists is a `CertificateAuthorityArn`
//! Private CA holds nothing under, which is that call's modelled 404 (covered by
//! `missing_resource_status_class`).
//!
//! So this suite mounts Private CA and EC2 beside this service on ONE engine — what the
//! assembled emulator does — and builds every CA and every VPC endpoint through those
//! services' own front doors, so what a row is checked against is their answer rather than
//! anything this suite seeded.
//!
//! Rows differ along what the rule generalises over: which reference cannot be served (the CA
//! or the endpoint), why it cannot (the status Private CA reports, where the ARN places the CA,
//! an id EC2 holds nothing under, an endpoint with no DNS entry), which status of the private
//! CA's own catalogue it is in, the FORM of a `VpcEndpointId` naming nothing (both lengths the
//! member's pattern allows), and which reference is at fault when two are.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use smithy_server_runtime::{EmbeddedEmulator, MultiServiceRouter};

use service_com_amazonaws_pcaconnectorscep_pcaconnectorscep as scep;

use crate::harness::{self, label};

/// The Region every connector in this suite is created in — not the emulator default, so
/// nothing can pass by a hardcoded default.
const REGION: &str = "eu-west-2";

/// A second Region this emulator also serves, for the row whose CA really exists but sits
/// outside the Region its connector is created in.
const OTHER_REGION: &str = "ap-northeast-1";

/// The status a freshly created connector is in until its transition comes due.
const STATUS_CREATING: &str = scep::state::STATUS_CREATING;
/// The settled status of a connector whose references all resolved.
const STATUS_ACTIVE: &str = scep::state::STATUS_ACTIVE;
/// The settled status of a connector whose creation did not come up.
const STATUS_FAILED: &str = scep::state::STATUS_FAILED;

/// Every value the model declares for `ConnectorStatus` / `ConnectorStatusReason`, so a row's
/// expectation is checked to be a value the wire enum accepts and not a string of this
/// suite's own (smithy_query shape=ConnectorStatus,ConnectorStatusReason).
const CONNECTOR_STATUS_VALUES: &[&str] = scep::ops::types::shared_types::CONNECTOR_STATUS_VALUES;
/// The `ConnectorStatusReason` catalogue, from the same place.
const CONNECTOR_STATUS_REASON_VALUES: &[&str] =
    scep::ops::types::shared_types::CONNECTOR_STATUS_REASON_VALUES;

/// The template that produces a ROOT CA's own certificate — step 2 of Private CA's documented
/// procedure for creating a root private CA, which is how this suite drives a CA to `ACTIVE`.
const ROOT_CA_TEMPLATE: &str = "arn:aws:acm-pca:::template/RootCACertificate/V1";

/// Make every transition Private CA owes come due at once, through that service's own state.
///
/// Strictly a clock move — it rewrites only the due instants (a CA's settle, an issued
/// certificate's availability), never a status or a stored value — so that service's own
/// handlers still compute what they report from their own records. A case neither sleeps for
/// one of those spans nor races it.
fn release_private_ca_clock(engine: &engine::InMemoryEngine) {
    let shared = service_com_amazonaws_acmpca_acmprivateca::state::service_state(engine)
        .expect("the private CA service's own state");
    let mut store = shared.lock();
    let already_due = SystemTime::now() - Duration::from_secs(1);
    for authority in store.certificate_authorities.values_mut() {
        authority.settles_at = already_due;
        for certificate in authority.certificates.values_mut() {
            certificate.available_at = already_due;
        }
    }
}

/// An emulator holding this service, Amazon Web Services Private CA and Amazon EC2 over one
/// shared engine, with a client for each of the two peer services over the router's own
/// in-process callback — the very seam this service resolves a reference through.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
    ec2: ::aws_sdk_ec2::Client,
    private_ca: ::aws_sdk_acmpca::Client,
    private_ca_elsewhere: ::aws_sdk_acmpca::Client,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signature verification is the emulator's own concern and is exercised elsewhere;
        // these cases are about these services' behaviour.
        router.set_request_authentication(false);
        router.register_service(scep::EmulatedService::new(Arc::clone(&engine)));
        router.register_service(
            service_com_amazonaws_acmpca_acmprivateca::EmulatedService::new(Arc::clone(&engine)),
        );
        router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
            Arc::clone(&engine),
        ));
        let callback = EmbeddedEmulator::from_router(router.clone());
        Self {
            ec2: ec2_client(&callback, REGION),
            private_ca: private_ca_client(&callback, REGION),
            private_ca_elsewhere: private_ca_client(&callback, OTHER_REGION),
            router,
            engine,
        }
    }

    /// One `CreateConnector` call, signed for [`REGION`].
    async fn create_connector(&self, request: Value) -> harness::Answer {
        harness::call(&self.router, REGION, "POST", "/connectors", Some(request)).await
    }

    /// The `Connector` `GetConnector` reports for `arn`.
    async fn get_connector(&self, arn: &str) -> harness::Answer {
        harness::call(
            &self.router,
            REGION,
            "GET",
            &format!("/connectors/{}", label(arn)),
            None,
        )
        .await
    }

    /// The `ConnectorSummary` `ListConnectors` reports for `arn`.
    async fn listed_connector(&self, case: &str, arn: &str) -> Value {
        let page = harness::call(&self.router, REGION, "GET", "/connectors", None).await;
        assert_eq!(
            page.status, 200,
            "case {case:?}: expected 200 from ListConnectors, got {} / {:?}",
            page.status, page.body
        );
        page.body["Connectors"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|entry| entry["Arn"] == json!(arn))
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: no ConnectorSummary for {arn:?} in {:?}",
                    page.body
                )
            })
            .clone()
    }

    /// A private CA of `region`, created through Private CA's own
    /// `CreateCertificateAuthority` and left where that call leaves it: existing, and waiting
    /// for the certificate that would put it in service.
    async fn pending_certificate_ca(&self, region: &str, common_name: &str) -> String {
        let client = self.client_for(region);
        let subject = ::aws_sdk_acmpca::types::Asn1Subject::builder()
            .common_name(common_name)
            .build();
        let configuration = ::aws_sdk_acmpca::types::CertificateAuthorityConfiguration::builder()
            .key_algorithm(::aws_sdk_acmpca::types::KeyAlgorithm::Rsa2048)
            .signing_algorithm(::aws_sdk_acmpca::types::SigningAlgorithm::Sha256Withrsa)
            .subject(subject)
            .build()
            .expect("the CA configuration builds");
        let created = client
            .create_certificate_authority()
            .certificate_authority_configuration(configuration)
            .certificate_authority_type(::aws_sdk_acmpca::types::CertificateAuthorityType::Root)
            .send()
            .await
            .expect("Private CA must create the certificate authority");
        let arn = created
            .certificate_authority_arn()
            .expect("Private CA must answer the CA's ARN")
            .to_string();
        // The CREATING -> PENDING_CERTIFICATE move that service owes the CA.
        release_private_ca_clock(&self.engine);
        arn
    }

    /// A private CA of [`REGION`] driven all the way to `ACTIVE` through Private CA's own
    /// documented procedure: its CSR is signed with the root template and the certificate is
    /// imported back.
    async fn active_ca(&self, common_name: &str) -> String {
        let arn = self.pending_certificate_ca(REGION, common_name).await;
        let csr = self
            .private_ca
            .get_certificate_authority_csr()
            .certificate_authority_arn(&arn)
            .send()
            .await
            .expect("Private CA must answer the CA's CSR")
            .csr()
            .expect("the CSR is present")
            .to_string();
        let issued = self
            .private_ca
            .issue_certificate()
            .certificate_authority_arn(&arn)
            .csr(::aws_smithy_types::Blob::new(csr.into_bytes()))
            .signing_algorithm(::aws_sdk_acmpca::types::SigningAlgorithm::Sha256Withrsa)
            .template_arn(ROOT_CA_TEMPLATE)
            .validity(
                ::aws_sdk_acmpca::types::Validity::builder()
                    .r#type(::aws_sdk_acmpca::types::ValidityPeriodType::Years)
                    .value(10)
                    .build()
                    .expect("the validity builds"),
            )
            .send()
            .await
            .expect("Private CA must issue the CA's own certificate");
        let certificate_arn = issued
            .certificate_arn()
            .expect("Private CA must answer the certificate's ARN")
            .to_string();
        // The instant that certificate becomes readable is one that service's clock owes.
        release_private_ca_clock(&self.engine);

        let certificate = self
            .private_ca
            .get_certificate()
            .certificate_authority_arn(&arn)
            .certificate_arn(&certificate_arn)
            .send()
            .await
            .expect("Private CA must answer the issued certificate")
            .certificate()
            .expect("the certificate is present")
            .to_string();
        self.private_ca
            .import_certificate_authority_certificate()
            .certificate_authority_arn(&arn)
            .certificate(::aws_smithy_types::Blob::new(certificate.into_bytes()))
            .send()
            .await
            .expect("Private CA must import the CA's certificate");
        arn
    }

    /// A private CA of [`REGION`] that was put in service and then DISABLED, which is one of
    /// the statuses it issues nothing in.
    async fn disabled_ca(&self, common_name: &str) -> String {
        let arn = self.active_ca(common_name).await;
        self.private_ca
            .update_certificate_authority()
            .certificate_authority_arn(&arn)
            .status(::aws_sdk_acmpca::types::CertificateAuthorityStatus::Disabled)
            .send()
            .await
            .expect("Private CA must disable the certificate authority");
        arn
    }

    /// A private CA of [`REGION`] that was DELETED and is inside its restoration period, the
    /// other status this suite can put a CA into through that service's own API.
    async fn deleted_ca(&self, common_name: &str) -> String {
        let arn = self.pending_certificate_ca(REGION, common_name).await;
        self.private_ca
            .delete_certificate_authority()
            .certificate_authority_arn(&arn)
            .send()
            .await
            .expect("Private CA must delete the certificate authority");
        arn
    }

    /// The status Private CA reports for `arn` — ASKED, never assumed, so a row's expectation
    /// rests on that service's own report of the CA this suite built.
    async fn reported_ca_status(&self, region: &str, arn: &str) -> String {
        self.client_for(region)
            .describe_certificate_authority()
            .certificate_authority_arn(arn)
            .send()
            .await
            .expect("Private CA must describe the certificate authority")
            .certificate_authority()
            .and_then(|authority| authority.status())
            .expect("Private CA must report the CA's status")
            .as_str()
            .to_string()
    }

    fn client_for(&self, region: &str) -> &::aws_sdk_acmpca::Client {
        if region == REGION {
            &self.private_ca
        } else {
            &self.private_ca_elsewhere
        }
    }

    /// A VPC of the account, created through EC2's own `CreateVpc`.
    async fn vpc(&self, cidr_block: &str) -> String {
        self.ec2
            .create_vpc()
            .cidr_block(cidr_block)
            .send()
            .await
            .expect("EC2 must create the VPC")
            .vpc()
            .and_then(::aws_sdk_ec2::types::Vpc::vpc_id)
            .expect("EC2 must answer the VPC's id")
            .to_string()
    }

    /// A VPC endpoint of the account, created through EC2's own `CreateVpcEndpoint`, as the id
    /// that service minted for it.
    async fn vpc_endpoint(
        &self,
        vpc_id: &str,
        endpoint_type: ::aws_sdk_ec2::types::VpcEndpointType,
        service_name: &str,
    ) -> String {
        self.ec2
            .create_vpc_endpoint()
            .vpc_id(vpc_id)
            .vpc_endpoint_type(endpoint_type)
            .service_name(service_name)
            // A VPC you create has DNS hostnames off, which a private hosted zone needs;
            // these endpoints are only references for a connector to resolve, so they ask for
            // no private DNS.
            .private_dns_enabled(false)
            .send()
            .await
            .expect("EC2 must create the VPC endpoint")
            .vpc_endpoint()
            .and_then(::aws_sdk_ec2::types::VpcEndpoint::vpc_endpoint_id)
            .expect("EC2 must answer the endpoint's id")
            .to_string()
    }

    /// How many DNS entries EC2 reports for `vpc_endpoint_id` — asked rather than assumed,
    /// since it is exactly what the rule under test turns on.
    async fn reported_dns_entries(&self, vpc_endpoint_id: &str) -> usize {
        self.ec2
            .describe_vpc_endpoints()
            .vpc_endpoint_ids(vpc_endpoint_id)
            .send()
            .await
            .expect("EC2 must describe the VPC endpoint")
            .vpc_endpoints()
            .iter()
            .find(|endpoint| endpoint.vpc_endpoint_id() == Some(vpc_endpoint_id))
            .expect("EC2 must report the endpoint it just created")
            .dns_entries()
            .len()
    }

    /// Remove `vpc_endpoint_id` through EC2's own `DeleteVpcEndpoints`.
    async fn delete_vpc_endpoint(&self, vpc_endpoint_id: &str) {
        self.ec2
            .delete_vpc_endpoints()
            .vpc_endpoint_ids(vpc_endpoint_id)
            .send()
            .await
            .expect("EC2 must delete the VPC endpoint");
    }
}

/// An EC2 client for `region` over `callback`, the in-process seam this emulator routes on: no
/// socket is opened and the endpoint URL is only the address that transport routes by.
fn ec2_client(callback: &EmbeddedEmulator, region: &str) -> ::aws_sdk_ec2::Client {
    let config = ::aws_sdk_ec2::Config::builder()
        .behavior_version(::aws_sdk_ec2::config::BehaviorVersion::latest())
        .region(::aws_sdk_ec2::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://ec2.{region}.amazonaws.com"))
        .credentials_provider(callback.aws_credentials_provider())
        .http_client(callback.http_client())
        .retry_config(::aws_sdk_ec2::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_ec2::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_ec2::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_ec2::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_ec2::Client::from_conf(config)
}

/// A Private CA client for `region`, over the same seam.
fn private_ca_client(callback: &EmbeddedEmulator, region: &str) -> ::aws_sdk_acmpca::Client {
    let config = ::aws_sdk_acmpca::Config::builder()
        .behavior_version(::aws_sdk_acmpca::config::BehaviorVersion::latest())
        .region(::aws_sdk_acmpca::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://acm-pca.{region}.amazonaws.com"))
        .credentials_provider(callback.aws_credentials_provider())
        .http_client(callback.http_client())
        .retry_config(::aws_sdk_acmpca::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_acmpca::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_acmpca::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_acmpca::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_acmpca::Client::from_conf(config)
}

/// One row: the references a create names, and the `ConnectorStatusReason` the connector must
/// settle with — `None` where every reference resolved, which settles it `ACTIVE` with no
/// reason at all.
#[derive(Debug)]
struct Row {
    name: &'static str,
    certificate_authority_arn: String,
    vpc_endpoint_id: Option<String>,
    reason: Option<&'static str>,
}

/// Drive one row: create the connector its references describe, and check what the creation
/// lands in through both readers once the transition has come due. Answers the connector's
/// ARN, so a case that needs the record afterwards does not create a second one.
async fn assert_row(emulator: &Emulator, row: &Row) -> String {
    let case = row.name;
    let expected_status = if row.reason.is_some() {
        STATUS_FAILED
    } else {
        STATUS_ACTIVE
    };
    assert!(
        CONNECTOR_STATUS_VALUES.contains(&expected_status),
        "case {case:?}: {expected_status:?} must be a declared ConnectorStatus, got \
         {CONNECTOR_STATUS_VALUES:?}"
    );
    if let Some(reason) = row.reason {
        assert!(
            CONNECTOR_STATUS_REASON_VALUES.contains(&reason),
            "case {case:?}: {reason:?} must be a declared ConnectorStatusReason, got \
             {CONNECTOR_STATUS_REASON_VALUES:?}"
        );
    }

    let mut request = json!({ "CertificateAuthorityArn": row.certificate_authority_arn });
    if let Some(vpc_endpoint_id) = row.vpc_endpoint_id.as_deref() {
        request["VpcEndpointId"] = json!(vpc_endpoint_id);
    }
    let created = emulator.create_connector(request).await;
    assert_eq!(
        (created.status, created.error_type.as_str()),
        (202, ""),
        "case {case:?}: the create is accepted whatever its references resolve to, so expected \
         202, got {} / {:?}",
        created.status,
        created.body
    );
    let arn = created.text("ConnectorArn");

    // Before the transition is due every connector reads the same: still provisioning, with
    // nothing said about why anything failed.
    harness::hold_pending(&emulator.engine, &arn);
    let creating = emulator.get_connector(&arn).await;
    assert_eq!(
        (creating.connector("Status"), creating.connector("StatusReason")),
        (&json!(STATUS_CREATING), &Value::Null),
        "case {case:?}: a connector whose transition is not due yet is {STATUS_CREATING} with no \
         StatusReason, got {:?}",
        creating.body
    );

    harness::release_pending(&emulator.engine, &arn);
    let settled = emulator.get_connector(&arn).await;
    let expected_reason = row.reason.map_or(Value::Null, |reason| json!(reason));
    assert_eq!(
        (settled.connector("Status"), settled.connector("StatusReason")),
        (&json!(expected_status), &expected_reason),
        "case {case:?}: expected {expected_status} with StatusReason {expected_reason:?}, got \
         {:?}",
        settled.body
    );

    // Both readers project one record, so the listing agrees with the single read.
    let listed = emulator.listed_connector(case, &arn).await;
    assert_eq!(
        (&listed["Status"], &listed["StatusReason"]),
        (settled.connector("Status"), settled.connector("StatusReason")),
        "case {case:?}: ListConnectors must report what GetConnector does, got {listed:?} against \
         {:?}",
        settled.body
    );

    // A connector that never came up serves nothing, and one that did serves: the status is
    // not decoration, it is what the next call is answered by.
    let challenge = harness::call(
        &emulator.router,
        REGION,
        "POST",
        "/challenges",
        Some(json!({ "ConnectorArn": arn })),
    )
    .await;
    let expected_challenge = if row.reason.is_some() {
        (400, "ValidationException")
    } else {
        (202, "")
    };
    assert_eq!(
        (challenge.status, challenge.error_type.as_str()),
        expected_challenge,
        "case {case:?}: a connector in {expected_status} must answer CreateChallenge with \
         {expected_challenge:?}, got {} / {:?}",
        challenge.status,
        challenge.body
    );
    arn
}

/// The whole rule, over one emulator whose CAs and VPC endpoints were all built through their
/// owners' own front doors.
#[tokio::test]
async fn a_connector_settles_by_what_the_services_that_own_its_references_report() {
    let emulator = Emulator::new();

    // The private CAs: one on its way into service, one in service, and the two statuses this
    // suite can take a CA out of service into through that service's own API.
    let pending = emulator
        .pending_certificate_ca(REGION, "scep-reference-pending")
        .await;
    let active = emulator.active_ca("scep-reference-active").await;
    let disabled = emulator.disabled_ca("scep-reference-disabled").await;
    let deleted = emulator.deleted_ca("scep-reference-deleted").await;
    let elsewhere = emulator
        .pending_certificate_ca(OTHER_REGION, "scep-reference-elsewhere")
        .await;

    // The statuses Private CA reports for them, named by that service's OWN vocabulary, so
    // every row below rests on its report rather than on what this suite believes it built.
    use service_com_amazonaws_acmpca_acmprivateca::state as pca;
    for (what, region, arn, expected) in [
        (
            "the CA on its way into service",
            REGION,
            &pending,
            pca::STATUS_PENDING_CERTIFICATE,
        ),
        ("the CA in service", REGION, &active, pca::STATUS_ACTIVE),
        ("the disabled CA", REGION, &disabled, pca::STATUS_DISABLED),
        ("the deleted CA", REGION, &deleted, pca::STATUS_DELETED),
        (
            "the CA of another Region",
            OTHER_REGION,
            &elsewhere,
            pca::STATUS_PENDING_CERTIFICATE,
        ),
    ] {
        let reported = emulator.reported_ca_status(region, arn).await;
        assert_eq!(
            reported, expected,
            "fixture {what:?}: Private CA must report {expected}, got {reported:?}"
        );
    }

    // The VPC endpoints: an interface endpoint, which publishes the names a client resolves it
    // by, and a gateway endpoint, which publishes none — asked of EC2, not assumed.
    let vpc = emulator.vpc("10.80.0.0/16").await;
    let interface = emulator
        .vpc_endpoint(
            &vpc,
            ::aws_sdk_ec2::types::VpcEndpointType::Interface,
            &format!("com.amazonaws.{REGION}.s3"),
        )
        .await;
    let gateway = emulator
        .vpc_endpoint(
            &vpc,
            ::aws_sdk_ec2::types::VpcEndpointType::Gateway,
            &format!("com.amazonaws.{REGION}.dynamodb"),
        )
        .await;
    assert!(
        emulator.reported_dns_entries(&interface).await > 0,
        "fixture: EC2 must report DNS entries for the interface endpoint {interface:?}"
    );
    assert_eq!(
        emulator.reported_dns_entries(&gateway).await,
        0,
        "fixture: EC2 must report no DNS entry for the gateway endpoint {gateway:?}"
    );

    let rows = vec![
        Row {
            name: "a CA on its way into service, reachable over the public internet",
            certificate_authority_arn: pending.clone(),
            vpc_endpoint_id: None,
            reason: None,
        },
        Row {
            name: "a CA in service, reachable over the public internet",
            certificate_authority_arn: active.clone(),
            vpc_endpoint_id: None,
            reason: None,
        },
        Row {
            name: "a CA its account disabled",
            certificate_authority_arn: disabled.clone(),
            vpc_endpoint_id: None,
            reason: Some(scep::state::REASON_PRIVATECA_INVALID_STATE),
        },
        Row {
            name: "a CA inside the restoration period of its delete",
            certificate_authority_arn: deleted.clone(),
            vpc_endpoint_id: None,
            reason: Some(scep::state::REASON_PRIVATECA_INVALID_STATE),
        },
        Row {
            name: "a CA in service, restricted to a VPC endpoint EC2 holds",
            certificate_authority_arn: active.clone(),
            vpc_endpoint_id: Some(interface.clone()),
            reason: None,
        },
        Row {
            name: "a VPC endpoint EC2 holds that publishes no DNS entry",
            certificate_authority_arn: active.clone(),
            vpc_endpoint_id: Some(gateway.clone()),
            reason: Some(scep::state::REASON_VPC_ENDPOINT_DNS_ENTRIES_NOT_FOUND),
        },
        Row {
            // The short form the member's pattern allows: `vpce-` and eight hexadecimal
            // digits (smithy_query shape=VpcEndpointId).
            name: "a short-form VPC endpoint id EC2 holds nothing under",
            certificate_authority_arn: pending.clone(),
            vpc_endpoint_id: Some("vpce-0123abcd".to_string()),
            reason: Some(scep::state::REASON_VPC_ENDPOINT_RESOURCE_NOT_FOUND),
        },
        Row {
            // The long form of the same pattern: seventeen hexadecimal digits, which is what
            // EC2 itself mints — so a well-formed id of the right length still names nothing.
            name: "a long-form VPC endpoint id EC2 holds nothing under",
            certificate_authority_arn: active.clone(),
            vpc_endpoint_id: Some("vpce-00112233445566778".to_string()),
            reason: Some(scep::state::REASON_VPC_ENDPOINT_RESOURCE_NOT_FOUND),
        },
        Row {
            // Two references, one of them unservable: the private CA is the resource a
            // connector exists to link, so it is the cause reported.
            name: "a CA of another Region beside a VPC endpoint EC2 holds",
            certificate_authority_arn: elsewhere.clone(),
            vpc_endpoint_id: Some(interface.clone()),
            reason: Some(scep::state::REASON_PRIVATECA_RESOURCE_NOT_FOUND),
        },
        Row {
            name: "a disabled CA beside a VPC endpoint id EC2 holds nothing under",
            certificate_authority_arn: disabled.clone(),
            vpc_endpoint_id: Some("vpce-aaaaaaaa".to_string()),
            reason: Some(scep::state::REASON_PRIVATECA_INVALID_STATE),
        },
    ];

    for row in &rows {
        assert_row(&emulator, row).await;
    }
}

/// The answer is a READ of the owning service's state, not a reading of the id's form: the same
/// `VpcEndpointId` brings a connector up while EC2 holds that endpoint and fails one once EC2
/// does not — and the connector that already settled keeps the outcome it settled with.
#[tokio::test]
async fn the_vpc_endpoint_outcome_follows_what_ec2_holds_when_the_create_is_made() {
    let emulator = Emulator::new();
    let ca = emulator
        .pending_certificate_ca(REGION, "scep-endpoint-read")
        .await;
    let vpc = emulator.vpc("10.90.0.0/16").await;
    let endpoint = emulator
        .vpc_endpoint(
            &vpc,
            ::aws_sdk_ec2::types::VpcEndpointType::Interface,
            &format!("com.amazonaws.{REGION}.s3"),
        )
        .await;

    let while_held = Row {
        name: "created while EC2 holds the endpoint",
        certificate_authority_arn: ca.clone(),
        vpc_endpoint_id: Some(endpoint.clone()),
        reason: None,
    };
    let came_up = assert_row(&emulator, &while_held).await;

    emulator.delete_vpc_endpoint(&endpoint).await;

    let once_gone = Row {
        name: "created once EC2 no longer holds the endpoint",
        certificate_authority_arn: ca.clone(),
        vpc_endpoint_id: Some(endpoint.clone()),
        reason: Some(scep::state::REASON_VPC_ENDPOINT_RESOURCE_NOT_FOUND),
    };
    assert_row(&emulator, &once_gone).await;

    // The earlier connector settled on what EC2 held at the time, and nothing re-decides it.
    let unchanged = emulator.get_connector(&came_up).await;
    assert_eq!(
        (
            unchanged.connector("Status"),
            unchanged.connector("StatusReason")
        ),
        (&json!(STATUS_ACTIVE), &Value::Null),
        "the connector that already settled keeps its outcome, got {:?}",
        unchanged.body
    );
}
