// SPDX-License-Identifier: CC0-1.0

//! End-to-end behaviour of the two HealthLake surfaces that are NOT awsJson1_0 RPC
//! calls: a data store's FHIR REST data plane (the wire its `DatastoreEndpoint`
//! advertises) and the data-transformation endpoint's
//! `POST /validate-source?sourceFormat=...` check.
//!
//! Both are driven through [`MultiServiceRouter::dispatch`] — the same entry point
//! `core/router` serves — as ordinary signed REST requests carrying NO
//! `X-Amz-Target`, which is how a client reaches them. The control-plane calls that
//! set a data store up go over the awsJson1_0 wire in the same test, so the endpoint
//! a FHIR request is sent to is the one the service itself advertised.
//!
//! The rules under test, each over a SET of inputs rather than one recorded case:
//!
//! * a FHIR `create` stores the submitted resource under a logical id the SERVER
//!   assigns, answers 201 with the stored resource, and a `read` of that id answers
//!   200 with the same bytes — for any resource type and any content;
//! * `/validate-source` answers 200 with a report whose `Valid` and
//!   `Summary.ErrorCount` are computed from the document's structure, for every
//!   content-level outcome, and refuses a `sourceFormat` it does not validate with
//!   400.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// Long enough that the `CREATING` -> `ACTIVE` transition
/// `crate::state::LIFECYCLE_TRANSITION` owes (500 ms) has fallen due.
const PAST_TRANSITION: Duration = Duration::from_millis(900);

/// The SigV4 credential scope every request below carries, which is what routes a
/// request with no `X-Amz-Target` to this service. The signature is not verified in
/// this in-process harness (no authenticator is registered); the scope is read for
/// routing exactly as the router reads a real client's.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/healthlake/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// The placeholder signature [`AUTHORIZATION`] carries, for a request whose credential
/// scope this harness varies ([`HealthLake::rpc_from`]). It is not verified here, for
/// the reason [`AUTHORIZATION`] states.
const ZERO_SIGNATURE: &str =
    "0000000000000000000000000000000000000000000000000000000000000000";

/// The host an SDK resolves this service's endpoint to, which every request below
/// carries unless a suite names another one.
pub(crate) const ENDPOINT_HOST: &str = "healthlake.us-east-1.amazonaws.com";

/// The emulated HealthLake service behind the production router.
///
/// `pub(crate)` so the sibling suites of this test target drive the service exactly the
/// way this one does, rather than each growing its own copy of the harness — including
/// `tests/all/restore_fhir_datastore.rs`, which needs BOTH wires (the awsJson1_0
/// control plane and a data store's FHIR REST endpoint).
pub(crate) struct HealthLake {
    router: MultiServiceRouter,
    /// The SINGLE shared engine the service was built from — the same store every
    /// handler and the FHIR data plane read and write.
    ///
    /// `pub(crate)` so a suite can read back a record no modelled operation
    /// REPORTS: `tests/all/datastore_encryption_and_preload.rs` reads the FHIR
    /// resources a preload delivered, because the FHIR `search` interaction that
    /// would enumerate them is still unimplemented (its own gap), and then reads
    /// each one over the endpoint.
    pub(crate) engine: Arc<engine::InMemoryEngine>,
}

impl HealthLake {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(
            service_com_amazonaws_healthlake_healthlake::EmulatedService::new(Arc::clone(&engine)),
        );
        Self { router, engine }
    }

    /// The same harness with the emulator's own Amazon S3 mounted beside this
    /// service on the ONE shared engine, exactly as the assembled router mounts
    /// them.
    ///
    /// `StartFHIRImportJob` reads the FHIR data its input location names out of
    /// that service, so the suite that drives an import
    /// (`tests/all/fhir_import_job_start.rs`) stages its input through Amazon
    /// S3's OWN operations and the import reads what Amazon S3 really holds.
    pub(crate) fn with_object_store() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(
            service_com_amazonaws_healthlake_healthlake::EmulatedService::new(Arc::clone(&engine)),
        );
        router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
            Arc::clone(&engine),
        ));
        Self { router, engine }
    }

    /// An Amazon S3 client over this emulator's own in-process callback — the
    /// seam an import job reads its input through, so an object staged with this
    /// client is the object an import reads.
    pub(crate) fn s3(&self) -> aws_sdk_s3::Client {
        s3_client_over(self.router.clone())
    }

    /// Dispatch one awsJson1_0 control-plane call.
    pub(crate) async fn rpc(&self, operation: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", ENDPOINT_HOST)
            .header("authorization", AUTHORIZATION)
            .header("content-type", "application/x-amz-json-1.0")
            .header("x-amz-target", format!("HealthLake.{operation}"))
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("the request builds");
        let response = self.router.dispatch(request).await;
        decoded(response)
    }

    /// Dispatch one REST request carrying no `X-Amz-Target`: a FHIR interaction, a
    /// `/validate-source` call or a tagging call on `/tags` or `/untag`, exactly as a
    /// signed client sends it.
    ///
    /// `pub(crate)` so the sibling suites drive a non-RPC surface through this one
    /// harness rather than each building its own router — `tests/all/rest_tagging_surface.rs`
    /// sends the data-transformation endpoint's tagging requests through it.
    pub(crate) async fn rest(
        &self,
        method: &str,
        uri: &str,
        content_type: &str,
        body: Vec<u8>,
    ) -> (u16, Value) {
        self.rest_from_host(Some(ENDPOINT_HOST), method, uri, content_type, body)
            .await
    }

    /// The same dispatch from a client that reached this emulator on `host` — or, with
    /// `None`, from one that sent no `Host` header at all.
    ///
    /// A non-RPC surface is addressed by HTTP method and path, and the host a client
    /// used to reach the endpoint is no part of that addressing, so
    /// `tests/all/data_transformation_endpoint_host.rs` varies it over the hosts a
    /// client may have been pointed at.
    pub(crate) async fn rest_from_host(
        &self,
        host: Option<&str>,
        method: &str,
        uri: &str,
        content_type: &str,
        body: Vec<u8>,
    ) -> (u16, Value) {
        let mut builder = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("authorization", AUTHORIZATION)
            .header("content-type", content_type);
        if let Some(host) = host {
            builder = builder.header("host", host);
        }
        let request = builder
            .body(bytes::Bytes::from(body))
            .expect("the request builds");
        let response = self.router.dispatch(request).await;
        decoded(response)
    }

    /// Dispatch a FHIR interaction, returning the status, the parsed body and the
    /// response headers a FHIR client reads.
    pub(crate) async fn fhir(
        &self,
        method: &str,
        uri: &str,
        body: &Value,
    ) -> (u16, Value, BTreeMap<String, String>) {
        let payload = if body.is_null() {
            Vec::new()
        } else {
            serde_json::to_vec(body).expect("the resource serializes")
        };
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", ENDPOINT_HOST)
            .header("authorization", AUTHORIZATION)
            .header("content-type", "application/fhir+json")
            .body(bytes::Bytes::from(payload))
            .expect("the request builds");
        let response = self.router.dispatch(request).await;
        let headers: BTreeMap<String, String> = response
            .headers()
            .iter()
            .map(|(name, value)| {
                (
                    name.as_str().to_ascii_lowercase(),
                    value.to_str().unwrap_or_default().to_string(),
                )
            })
            .collect();
        let (status, payload) = decoded(response);
        (status, payload, headers)
    }

    /// Create a data store, wait out its lifecycle transition, and return the FHIR
    /// endpoint PATH `DescribeFHIRDatastore` advertises for it.
    pub(crate) async fn active_datastore(&self, name: &str) -> (String, String) {
        let (status, created) = self
            .rpc(
                "CreateFHIRDatastore",
                &json!({ "DatastoreTypeVersion": "R4", "DatastoreName": name }),
            )
            .await;
        assert_eq!(
            status, 200,
            "CreateFHIRDatastore must answer 200, got {status}: {created}"
        );
        let id = created["DatastoreId"]
            .as_str()
            .unwrap_or_else(|| panic!("the create reports a DatastoreId, got {created}"))
            .to_string();
        tokio::time::sleep(PAST_TRANSITION).await;
        let (status, described) = self
            .rpc("DescribeFHIRDatastore", &json!({ "DatastoreId": id }))
            .await;
        assert_eq!(
            status, 200,
            "DescribeFHIRDatastore must answer 200, got {status}: {described}"
        );
        let properties = &described["DatastoreProperties"];
        assert_eq!(
            properties["DatastoreStatus"].as_str(),
            Some("ACTIVE"),
            "the data store must be ACTIVE before its FHIR endpoint is used, got {properties}"
        );
        let endpoint = properties["DatastoreEndpoint"]
            .as_str()
            .unwrap_or_else(|| panic!("the data store reports a DatastoreEndpoint, got {properties}"))
            .to_string();
        (id, endpoint_path(&endpoint))
    }

    /// Look at `DescribeFHIRDatastore` until the data store `id` reports `ACTIVE`, and
    /// answer the `DatastoreProperties` of that read.
    ///
    /// A lifecycle transition this service owes is materialised by whichever call reads
    /// the record next, so waiting for one means LOOKING again until it holds — never
    /// waiting for a span, which would tie the outcome to how fast the host runs. The
    /// deadline is the one every suite waits by
    /// ([`crate::fhir_datastore_lifecycle::SETTLING_DEADLINE`]).
    pub(crate) async fn poll_until_active(&self, id: &str) -> Value {
        let deadline =
            std::time::Instant::now() + crate::fhir_datastore_lifecycle::SETTLING_DEADLINE;
        loop {
            let (status, described) = self
                .rpc("DescribeFHIRDatastore", &json!({ "DatastoreId": id }))
                .await;
            assert_eq!(
                status, 200,
                "DescribeFHIRDatastore must answer 200, got {status}: {described}"
            );
            let properties = &described["DatastoreProperties"];
            if properties["DatastoreStatus"] == "ACTIVE" {
                return properties.clone();
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the data store {id} did not reach ACTIVE within the settling deadline, \
                 last read {described}"
            );
            tokio::task::yield_now().await;
        }
    }

    /// The same harness as [`HealthLake::new`], answering as an emulator a client
    /// reached at `base` — `scheme://host[:port]`, the address its data port is
    /// published on.
    ///
    /// A router that was never served on a port reports the framework's
    /// [`smithy_server_runtime::UNSERVED_URL_BASE`], so a value built from the emulator's
    /// own address and one built from a constant look alike. This seam names another
    /// base, which makes what an address is built FROM observable.
    pub(crate) fn served_at(base: &str) -> Self {
        let mut service = Self::new();
        service.router.set_url_base(
            smithy_server_runtime::EndpointUrl::parse(base)
                .unwrap_or_else(|| panic!("{base:?} is a usable emulator base URL")),
        );
        service
    }

    /// Dispatch one awsJson1_0 control-plane call from a client that reached this
    /// emulator on `host`, with a credential scoped to `region`.
    ///
    /// Both are what an SDK pointed at the real service would carry, and neither is
    /// part of the address this emulator is reached on.
    pub(crate) async fn rpc_from(
        &self,
        host: &str,
        region: &str,
        operation: &str,
        body: &Value,
    ) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", host)
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/healthlake/\
                     aws4_request, SignedHeaders=host;x-amz-date, Signature={ZERO_SIGNATURE}"
                ),
            )
            .header("content-type", "application/x-amz-json-1.0")
            .header("x-amz-target", format!("HealthLake.{operation}"))
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("the request builds");
        decoded(self.router.dispatch(request).await)
    }

    /// Dispatch a FHIR interaction to the ABSOLUTE URL a client was handed: the
    /// published `DatastoreEndpoint` with the interaction joined onto it, sent exactly
    /// as a client that follows that value sends it — the whole URL on the request
    /// line and the authority it names in `Host`.
    ///
    /// The sibling [`HealthLake::fhir`] sends the PATH alone, which says nothing about
    /// the host a published endpoint announced; this one dispatches the announced
    /// address itself, so what is under test is whether that address is one this
    /// emulator answers on.
    pub(crate) async fn fhir_at_url(
        &self,
        method: &str,
        url: &str,
        body: &Value,
    ) -> (u16, Value) {
        let authority = url
            .split_once("://")
            .map(|(_scheme, rest)| rest.split('/').next().unwrap_or(rest))
            .unwrap_or_else(|| panic!("{url:?} must be an absolute URL"));
        let payload = if body.is_null() {
            Vec::new()
        } else {
            serde_json::to_vec(body).expect("the resource serializes")
        };
        let request = http::Request::builder()
            .method(method)
            .uri(url)
            .header("host", authority)
            .header("authorization", AUTHORIZATION)
            .header("content-type", "application/fhir+json")
            .body(bytes::Bytes::from(payload))
            .expect("the request builds");
        decoded(self.router.dispatch(request).await)
    }
}

/// The status and the parsed body of a dispatched response.
fn decoded(response: http::Response<bytes::Bytes>) -> (u16, Value) {
    let status = response.status().as_u16();
    let payload = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, payload)
}

/// The PATH of an advertised `DatastoreEndpoint`, which is what a client sends its
/// FHIR interactions to. Taking it from the advertised endpoint — rather than writing
/// it out here — is what ties the served path space to the one the service publishes.
pub(crate) fn endpoint_path(endpoint: &str) -> String {
    let after_scheme = endpoint
        .split_once("://")
        .map_or(endpoint, |(_scheme, rest)| rest);
    let path = after_scheme
        .find('/')
        .map_or("/", |start| &after_scheme[start..]);
    path.to_string()
}

/// An Amazon S3 client over one emulator's own in-process callback: the seam an
/// import job reads its input through and an export job writes its output through,
/// so an object this client holds is the object those jobs see.
///
/// The ONE place that configuration lives, so the suites that mount Amazon S3 beside
/// this service — whichever harness they build their router with — cannot drift into
/// two different clients.
pub(crate) fn s3_client_over(router: MultiServiceRouter) -> aws_sdk_s3::Client {
    let callback = smithy_server_runtime::EmbeddedEmulator::from_router(router);
    let config = aws_sdk_s3::Config::builder()
        .behavior_version(aws_sdk_s3::config::BehaviorVersion::latest())
        .region(aws_sdk_s3::config::Region::new("us-east-1"))
        .endpoint_url("https://s3.us-east-1.amazonaws.com".to_string())
        .force_path_style(true)
        .credentials_provider(callback.aws_credentials_provider())
        .http_client(callback.http_client())
        .retry_config(aws_sdk_s3::config::retry::RetryConfig::disabled())
        .timeout_config(aws_sdk_s3::config::timeout::TimeoutConfig::disabled())
        .identity_cache(aws_sdk_s3::config::IdentityCache::no_cache())
        .stalled_stream_protection(aws_sdk_s3::config::StalledStreamProtectionConfig::disabled())
        .build();
    aws_sdk_s3::Client::from_conf(config)
}

/// The `issue[0].code` of an `OperationOutcome` body.
pub(crate) fn outcome_code(body: &Value) -> String {
    assert_eq!(
        body["resourceType"].as_str(),
        Some("OperationOutcome"),
        "a FHIR failure is an OperationOutcome, got {body}"
    );
    body["issue"][0]["code"]
        .as_str()
        .unwrap_or("<no code>")
        .to_string()
}

/// The modelled shape name in an awsJson error envelope's `__type`.
fn error_code(payload: &Value) -> String {
    payload["__type"]
        .as_str()
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// A FHIR create of `resource` against `endpoint_path`, asserted to have succeeded,
/// returning the stored resource.
pub(crate) async fn created(
    service: &HealthLake,
    path: &str,
    resource_type: &str,
    resource: &Value,
) -> Value {
    let (status, body, headers) = service
        .fhir("POST", &format!("{path}{resource_type}"), resource)
        .await;
    assert_eq!(
        status, 201,
        "case {resource_type}: a FHIR create answers 201, got {status}: {body}"
    );
    assert_eq!(
        headers.get("content-type").map(String::as_str),
        Some("application/fhir+json"),
        "case {resource_type}: a FHIR response carries the FHIR media type, got {headers:?}"
    );
    let id = body["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {resource_type}: the create assigns an id, got {body}"));
    assert!(
        headers
            .get("location")
            .is_some_and(|location| location.contains(id)),
        "case {resource_type}: the create reports a Location naming the new resource, \
         got {headers:?}"
    );
    body
}

/// The elements of a C-CDA Continuity of Care Document this suite builds a source
/// document from, in document order.
///
/// Source: HL7 CDA Release 2's `ClinicalDocument` class — these are exactly the
/// children it declares with a minimum cardinality of 1, which C-CDA R2.1 inherits.
/// A row omits one or more of them to make a document INCOMPLETE.
fn ccd_element(name: &str) -> String {
    match name {
        "typeId" => r#"<typeId root="2.16.840.1.113883.1.3" extension="POCD_HD000040"/>"#.to_string(),
        "id" => r#"<id root="2.16.840.1.113883.19.5" extension="ccd-1"/>"#.to_string(),
        "code" => r#"<code code="34133-9" codeSystem="2.16.840.1.113883.6.1" displayName="Summarization of Episode Note"/>"#.to_string(),
        "effectiveTime" => r#"<effectiveTime value="20260101120000+0000"/>"#.to_string(),
        "confidentialityCode" => {
            r#"<confidentialityCode code="N" codeSystem="2.16.840.1.113883.5.25"/>"#.to_string()
        }
        "recordTarget" => r#"<recordTarget><patientRole><id root="2.16.840.1.113883.19.5" extension="p-1"/><patient><name><given>Ada</given><family>Lovelace</family></name><administrativeGenderCode code="F" codeSystem="2.16.840.1.113883.5.1"/><birthTime value="19151210"/></patient></patientRole></recordTarget>"#.to_string(),
        "author" => r#"<author><time value="20260101120000+0000"/><assignedAuthor><id root="2.16.840.1.113883.19.5" extension="a-1"/></assignedAuthor></author>"#.to_string(),
        "custodian" => r#"<custodian><assignedCustodian><representedCustodianOrganization><id root="2.16.840.1.113883.19.5"/><name>Emulated Clinic</name></representedCustodianOrganization></assignedCustodian></custodian>"#.to_string(),
        other => panic!("the suite builds no C-CDA element {other}"),
    }
}

/// Every required `ClinicalDocument` child except the document body, in order.
const CCD_HEADER_ELEMENTS: &[&str] = &[
    "typeId",
    "id",
    "code",
    "effectiveTime",
    "confidentialityCode",
    "recordTarget",
    "author",
    "custodian",
];

/// A C-CDA CCD carrying every required header element except those in `omit`, and the
/// body `body` (`None` leaves the document with no `component` at all).
fn ccd(omit: &[&str], body: Option<&str>) -> String {
    let header: String = CCD_HEADER_ELEMENTS
        .iter()
        .filter(|name| !omit.contains(*name))
        .map(|name| ccd_element(name))
        .collect();
    let component = body.map(|body| format!("<component>{body}</component>"));
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><ClinicalDocument xmlns="urn:hl7-org:v3">{header}{}</ClinicalDocument>"#,
        component.unwrap_or_default()
    )
}

/// A `structuredBody` carrying `sections` sections.
fn structured_body(sections: usize) -> String {
    let inner: String = (0..sections)
        .map(|index| {
            format!(
                r#"<component><section><title>Section {index}</title><text>Narrative {index}</text></section></component>"#
            )
        })
        .collect();
    format!("<structuredBody>{inner}</structuredBody>")
}

/// A complete C-CDA CCD: the positive control.
fn complete_ccd() -> String {
    ccd(&[], Some(&structured_body(2)))
}

#[tokio::test]
async fn a_fhir_create_assigns_an_id_and_a_read_answers_the_stored_resource() {
    let service = HealthLake::new();
    let (_id, path) = service.active_datastore("fhir-roundtrip").await;

    // The rule generalises over the resource TYPE and over the submitted content, so
    // the rows differ along both: a small resource, one with nested objects and
    // arrays, one with numeric and boolean leaves, and one with a client-supplied
    // `id` and `meta` the server must not honour.
    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "a minimal Patient",
            "Patient",
            json!({
                "resourceType": "Patient",
                "name": [{ "family": "Lovelace", "given": ["Ada"] }],
                "identifier": [{ "system": "urn:oid:2.16.840.1.113883.19.5", "value": "p-1" }],
            }),
        ),
        (
            "an Observation with nested content",
            "Observation",
            json!({
                "resourceType": "Observation",
                "status": "final",
                "code": { "coding": [{ "system": "http://loinc.org", "code": "8867-4" }] },
                "valueQuantity": { "value": 72, "unit": "beats/minute" },
            }),
        ),
        (
            "a Practitioner with scalar leaves of several JSON types",
            "Practitioner",
            json!({
                "resourceType": "Practitioner",
                "active": true,
                "name": [{ "family": "Hopper", "given": ["Grace", "Brewster"] }],
                "qualification": [{ "identifier": [{ "value": "1" }] }],
            }),
        ),
        (
            "a resource carrying a client-chosen id and meta",
            "Patient",
            json!({
                "resourceType": "Patient",
                "id": "client-chosen-id",
                "meta": { "versionId": "99", "lastUpdated": "2000-01-01T00:00:00Z", "profile": ["http://example.org/fhir/StructureDefinition/demo"] },
                "name": [{ "family": "Turing" }],
            }),
        ),
    ];

    for (case, resource_type, submitted) in rows {
        let stored = created(&service, &path, resource_type, &submitted).await;
        let id = stored["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the create assigns an id, got {stored}"))
            .to_string();

        // The id is the SERVER's, never the one the request carried.
        if let Some(requested) = submitted.get("id").and_then(Value::as_str) {
            assert_ne!(
                id, requested,
                "case {case:?}: FHIR has the server assign the logical id, but the \
                 client-chosen {requested:?} was honoured"
            );
        }
        assert_eq!(
            stored["meta"]["versionId"].as_str(),
            Some("1"),
            "case {case:?}: a created resource is at version 1, got {stored}"
        );
        assert!(
            stored["meta"]["lastUpdated"].as_str().is_some_and(|at| at.ends_with('Z')),
            "case {case:?}: a created resource carries meta.lastUpdated, got {stored}"
        );
        // Everything the caller submitted except the two values the server owns is
        // echoed back unchanged — including the rest of `meta`.
        for (member, value) in submitted.as_object().expect("the row is an object") {
            if member == "id" || member == "meta" {
                continue;
            }
            assert_eq!(
                &stored[member], value,
                "case {case:?}: the created resource must echo {member}, got {stored}"
            );
        }
        if let Some(profile) = submitted["meta"].get("profile") {
            assert_eq!(
                &stored["meta"]["profile"], profile,
                "case {case:?}: meta members the server does not own are kept, got {stored}"
            );
        }

        // ROUND TRIP: the read answers exactly what the create stored.
        let (status, read, headers) = service
            .fhir("GET", &format!("{path}{resource_type}/{id}"), &Value::Null)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a FHIR read answers 200, got {status}: {read}"
        );
        assert_eq!(
            read, stored,
            "case {case:?}: a read must answer the stored resource, got {read}"
        );
        assert_eq!(
            headers.get("etag").map(String::as_str),
            Some("W/\"1\""),
            "case {case:?}: a read reports the stored version as an ETag, got {headers:?}"
        );
    }
}

#[tokio::test]
async fn two_creates_of_the_same_content_are_two_resources() {
    let service = HealthLake::new();
    let (_id, path) = service.active_datastore("fhir-distinct-ids").await;
    let submitted = json!({ "resourceType": "Patient", "name": [{ "family": "Noether" }] });

    let mut ids = Vec::new();
    for attempt in 0..3 {
        let stored = created(&service, &path, "Patient", &submitted).await;
        let id = stored["id"]
            .as_str()
            .unwrap_or_else(|| panic!("attempt {attempt}: the create assigns an id"))
            .to_string();
        assert!(
            !ids.contains(&id),
            "attempt {attempt}: a create mints a new logical id, got the already-used {id:?} \
             (ids so far {ids:?})"
        );
        ids.push(id);
    }
    // Each one is readable on its own, so no create overwrote an earlier resource.
    for id in &ids {
        let (status, read, _headers) = service
            .fhir("GET", &format!("{path}Patient/{id}"), &Value::Null)
            .await;
        assert_eq!(
            status, 200,
            "every created resource stays readable: {id} answered {status}: {read}"
        );
    }
}

#[tokio::test]
async fn a_fhir_interaction_states_its_refusals_as_an_operation_outcome() {
    let service = HealthLake::new();
    let (datastore_id, path) = service.active_datastore("fhir-refusals").await;
    let stored = created(
        &service,
        &path,
        "Patient",
        &json!({ "resourceType": "Patient" }),
    )
    .await;
    let id = stored["id"].as_str().expect("the create assigns an id");

    // (case, method, uri, body, expected status, expected OperationOutcome code)
    let rows: Vec<(&str, &str, String, Value, u16, &str)> = vec![
        (
            "a read of an id the data store holds nothing under",
            "GET",
            format!("{path}Patient/00000000-0000-4000-8000-000000000000"),
            Value::Null,
            404,
            "not-found",
        ),
        (
            "a read under another resource type",
            "GET",
            format!("{path}Observation/{id}"),
            Value::Null,
            404,
            "not-found",
        ),
        (
            "a path segment that is no FHIR resource type",
            "POST",
            format!("{path}patient"),
            json!({ "resourceType": "patient" }),
            404,
            "not-found",
        ),
        (
            "a create whose resourceType disagrees with the URL",
            "POST",
            format!("{path}Patient"),
            json!({ "resourceType": "Observation", "status": "final" }),
            400,
            "invalid",
        ),
        (
            "a create of a resource carrying no resourceType",
            "POST",
            format!("{path}Patient"),
            json!({ "name": [{ "family": "Curie" }] }),
            400,
            "structure",
        ),
        (
            "a create of a body that is not a JSON object",
            "POST",
            format!("{path}Patient"),
            json!(["Patient"]),
            400,
            "structure",
        ),
        (
            "a create with no body at all",
            "POST",
            format!("{path}Patient"),
            Value::Null,
            400,
            "structure",
        ),
        (
            "an interaction on a data store that does not exist",
            "GET",
            "/datastore/ffffffffffffffffffffffffffffffff/r4/Patient/1".to_string(),
            Value::Null,
            404,
            "not-found",
        ),
        (
            "an update whose body names a different id from the URL",
            "PUT",
            format!("{path}Patient/{id}"),
            json!({ "resourceType": "Patient", "id": "some-other-id" }),
            400,
            "invalid",
        ),
        (
            "an update at an id outside the FHIR id type",
            "PUT",
            format!("{path}Patient/patient_1"),
            json!({ "resourceType": "Patient" }),
            400,
            "invalid",
        ),
        (
            "a delete, which this endpoint does not implement yet",
            "DELETE",
            format!("{path}Patient/{id}"),
            Value::Null,
            501,
            "not-supported",
        ),
        (
            "a search parameter this endpoint does not implement yet",
            "GET",
            format!("{path}Patient?name=Lovelace"),
            Value::Null,
            400,
            "not-supported",
        ),
        (
            "a version read, which this endpoint does not implement yet",
            "GET",
            format!("{path}Patient/{id}/_history/1"),
            Value::Null,
            501,
            "not-supported",
        ),
    ];

    for (case, method, uri, body, expected_status, expected_code) in rows {
        let (status, answered, headers) = service.fhir(method, &uri, &body).await;
        assert_eq!(
            status, expected_status,
            "case {case:?} ({method} {uri}): expected HTTP {expected_status}, got {status}: \
             {answered}"
        );
        let code = outcome_code(&answered);
        assert_eq!(
            code, expected_code,
            "case {case:?}: expected the OperationOutcome code {expected_code:?}, got \
             {code:?} in {answered}"
        );
        assert_eq!(
            headers.get("content-type").map(String::as_str),
            Some("application/fhir+json"),
            "case {case:?}: an OperationOutcome is FHIR JSON, got {headers:?}"
        );
    }

    // Nothing above changed what the data store holds.
    let (status, read, _headers) = service
        .fhir("GET", &format!("{path}Patient/{id}"), &Value::Null)
        .await;
    assert_eq!(
        status, 200,
        "the stored resource is still readable after every refusal, got {status}: {read}"
    );
    assert_eq!(
        read, stored,
        "a refused interaction must not change the stored resource, got {read}"
    );
    assert!(
        !datastore_id.is_empty(),
        "the suite created a data store to address"
    );
}

#[tokio::test]
async fn one_data_stores_resources_are_not_readable_through_another() {
    let service = HealthLake::new();
    let (_first_id, first) = service.active_datastore("fhir-isolation-a").await;
    let (_second_id, second) = service.active_datastore("fhir-isolation-b").await;
    assert_ne!(
        first, second,
        "two data stores advertise two endpoints, got {first} twice"
    );

    let stored = created(
        &service,
        &first,
        "Patient",
        &json!({ "resourceType": "Patient", "name": [{ "family": "Franklin" }] }),
    )
    .await;
    let id = stored["id"].as_str().expect("the create assigns an id");

    let (status, answered, _headers) = service
        .fhir("GET", &format!("{second}Patient/{id}"), &Value::Null)
        .await;
    assert_eq!(
        status, 404,
        "a resource of one data store is not readable through another, got {status}: {answered}"
    );
    assert_eq!(
        outcome_code(&answered),
        "not-found",
        "the refusal names the FHIR not-found code, got {answered}"
    );

    let (status, read, _headers) = service
        .fhir("GET", &format!("{first}Patient/{id}"), &Value::Null)
        .await;
    assert_eq!(
        status, 200,
        "the resource stays readable through its own data store, got {status}: {read}"
    );
}

#[tokio::test]
async fn the_data_plane_bounds_what_it_retains_by_count_and_by_bytes() {
    // Every ceiling the FHIR create path enforces, in one data store: the size of ONE
    // resource, the BYTES one data store retains, and the COUNT it holds. Each is
    // refused rather than evicting, and a resource created earlier stays readable
    // after every refusal.
    let service = HealthLake::new();
    let (_id, path) = service.active_datastore("fhir-bounds").await;

    let first = created(
        &service,
        &path,
        "Patient",
        &json!({ "resourceType": "Patient", "name": [{ "family": "Germain" }] }),
    )
    .await;
    let first_id = first["id"].as_str().expect("the create assigns an id").to_string();

    // One resource past the per-resource byte ceiling (1 MiB) is refused BEFORE it is
    // parsed, with the FHIR code for content that is too long.
    let oversize = json!({
        "resourceType": "Patient",
        "name": [{ "family": "x".repeat(1 << 20) }],
    });
    let (status, answered, _headers) = service
        .fhir("POST", &format!("{path}Patient"), &oversize)
        .await;
    assert_eq!(
        status, 413,
        "a resource past the per-resource byte ceiling is refused, got {status}: {answered}"
    );
    assert_eq!(
        outcome_code(&answered),
        "too-long",
        "the refusal names the FHIR too-long code, got {answered}"
    );

    // Resources just under that ceiling fill the per-data-store byte budget (16 MiB),
    // and the create that would pass it is refused with the retention code. The loop
    // is bounded: 16 MiB of 1 MiB-ish resources is at most 17 creates.
    let chunky = json!({
        "resourceType": "Observation",
        "status": "final",
        "note": [{ "text": "y".repeat((1 << 20) - 256) }],
    });
    let mut accepted = 0;
    let refused = loop {
        let (status, answered, _headers) = service
            .fhir("POST", &format!("{path}Observation"), &chunky)
            .await;
        if status != 201 {
            break (status, answered);
        }
        accepted += 1;
        assert!(
            accepted <= 17,
            "the per-data-store byte budget must refuse a create before {accepted} \
             resources of ~1 MiB are retained"
        );
    };
    assert_eq!(
        refused.0, 507,
        "a create past the per-data-store byte budget is refused, got {}: {}",
        refused.0, refused.1
    );
    assert_eq!(
        outcome_code(&refused.1),
        "too-costly",
        "the refusal names the FHIR too-costly code, got {}",
        refused.1
    );
    assert!(
        accepted > 0,
        "the budget admits resources before it refuses one, accepted {accepted}"
    );

    // The resource stored first is still readable after every refusal above: the
    // ceilings refuse, they never evict.
    let (status, read, _headers) = service
        .fhir("GET", &format!("{path}Patient/{first_id}"), &Value::Null)
        .await;
    assert_eq!(
        status, 200,
        "a refused create must not evict an earlier resource, got {status}: {read}"
    );
    assert_eq!(
        read, first,
        "the earlier resource is unchanged, got {read}"
    );
}

#[tokio::test]
async fn the_data_plane_bounds_how_many_resources_one_data_store_holds() {
    // The COUNT ceiling (500), reached with resources small enough that the byte
    // budget is not what refuses: a tiny resource is ~100 bytes, so 500 of them are
    // far under 16 MiB.
    let service = HealthLake::new();
    let (_id, path) = service.active_datastore("fhir-count-bound").await;
    let tiny = json!({ "resourceType": "Patient" });

    let mut accepted = 0;
    let refused = loop {
        let (status, answered, _headers) =
            service.fhir("POST", &format!("{path}Patient"), &tiny).await;
        if status != 201 {
            break (status, answered);
        }
        accepted += 1;
        assert!(
            accepted <= 500,
            "the count ceiling must refuse the create after 500 resources, accepted \
             {accepted}"
        );
    };
    assert_eq!(
        accepted, 500,
        "the data store admits its whole count ceiling before refusing, accepted {accepted}"
    );
    assert_eq!(
        refused.0, 507,
        "a create past the count ceiling is refused, got {}: {}",
        refused.0, refused.1
    );
    assert_eq!(
        outcome_code(&refused.1),
        "too-costly",
        "the refusal names the FHIR too-costly code, got {}",
        refused.1
    );
}

#[tokio::test]
async fn a_datastore_serves_its_fhir_endpoint_only_while_it_is_active() {
    let service = HealthLake::new();
    // Taken BEFORE the transition falls due, so the endpoint is addressed while the
    // data store is still CREATING.
    let (status, created_store) = service
        .rpc(
            "CreateFHIRDatastore",
            &json!({ "DatastoreTypeVersion": "R4", "DatastoreName": "fhir-while-creating" }),
        )
        .await;
    assert_eq!(status, 200, "the create answers 200, got {created_store}");
    assert_eq!(
        created_store["DatastoreStatus"].as_str(),
        Some("CREATING"),
        "a new data store starts CREATING, got {created_store}"
    );
    let path = endpoint_path(
        created_store["DatastoreEndpoint"]
            .as_str()
            .expect("the create reports a DatastoreEndpoint"),
    );

    let (status, answered, _headers) = service
        .fhir(
            "POST",
            &format!("{path}Patient"),
            &json!({ "resourceType": "Patient" }),
        )
        .await;
    assert_eq!(
        status, 404,
        "a data store that is not ACTIVE serves no FHIR interaction, got {status}: {answered}"
    );
    assert_eq!(
        outcome_code(&answered),
        "not-found",
        "the refusal names the FHIR not-found code, got {answered}"
    );

    // Once the transition the clock owed has fallen due, the same request is served.
    tokio::time::sleep(PAST_TRANSITION).await;
    let (status, answered, _headers) = service
        .fhir(
            "POST",
            &format!("{path}Patient"),
            &json!({ "resourceType": "Patient" }),
        )
        .await;
    assert_eq!(
        status, 201,
        "an ACTIVE data store serves the create, got {status}: {answered}"
    );
}

#[tokio::test]
async fn validate_source_reports_a_documents_structure_at_http_200() {
    let service = HealthLake::new();

    // (case, document, expected issue count)
    //
    // The rows span the class the rule generalises over: a complete document, each
    // kind of structural shortfall (a missing header element, several at once, a body
    // with no section, no body at all), both legal body forms, and a body that is not
    // a CDA document at all.
    let rows: Vec<(&str, String, usize)> = vec![
        ("a complete C-CDA CCD", complete_ccd(), 0),
        (
            "a CCD with a single section",
            ccd(&[], Some(&structured_body(1))),
            0,
        ),
        (
            "a CCD with a non-XML body",
            ccd(
                &[],
                Some(r#"<nonXMLBody><text mediaType="text/plain">report</text></nonXMLBody>"#),
            ),
            0,
        ),
        (
            "a CCD with no recordTarget",
            ccd(&["recordTarget"], Some(&structured_body(2))),
            1,
        ),
        (
            "a CCD with no custodian",
            ccd(&["custodian"], Some(&structured_body(2))),
            1,
        ),
        (
            "a CCD missing three header elements",
            ccd(
                &["typeId", "author", "confidentialityCode"],
                Some(&structured_body(1)),
            ),
            3,
        ),
        (
            "a CCD whose structuredBody carries no section",
            ccd(&[], Some(&structured_body(0))),
            1,
        ),
        (
            "a CCD whose component carries no body",
            ccd(&[], Some("")),
            1,
        ),
        ("a CCD with no component at all", ccd(&[], None), 1),
        (
            "an empty C-CDA header with no content",
            ccd(CCD_HEADER_ELEMENTS, None),
            CCD_HEADER_ELEMENTS.len() + 1,
        ),
        (
            "a well-formed XML document that is not CDA",
            r#"<?xml version="1.0"?><html><body>not a document</body></html>"#.to_string(),
            1,
        ),
        (
            "a ClinicalDocument in the wrong namespace",
            r#"<ClinicalDocument xmlns="urn:example:v1"/>"#.to_string(),
            1,
        ),
        (
            "a truncated C-CDA document",
            complete_ccd()
                .chars()
                .take(complete_ccd().chars().count() / 2)
                .collect(),
            1,
        ),
        ("bytes that are not XML at all", "not xml at all".to_string(), 1),
    ];

    for (case, document, expected_issues) in rows {
        let (status, report) = service
            .rest(
                "POST",
                "/validate-source?sourceFormat=CCDA",
                "application/octet-stream",
                document.into_bytes(),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a content-level outcome is reported at HTTP 200, got {status}: \
             {report}"
        );
        assert_eq!(
            report["SourceFormat"].as_str(),
            Some("CCDA"),
            "case {case:?}: the report names the validated source format, got {report}"
        );
        let issues = report["Issues"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: the report lists Issues, got {report}"));
        assert_eq!(
            issues.len(),
            expected_issues,
            "case {case:?}: expected {expected_issues} issue(s), got {issues:?}"
        );
        assert_eq!(
            report["Summary"]["ErrorCount"].as_u64(),
            Some(expected_issues as u64),
            "case {case:?}: Summary.ErrorCount counts the issues, got {report}"
        );
        assert_eq!(
            report["Summary"]["WarningCount"].as_u64(),
            Some(0),
            "case {case:?}: ValidationSummary requires WarningCount, and every rule this \
             validator applies is an error, got {report}"
        );
        assert_eq!(
            report["Valid"].as_bool(),
            Some(expected_issues == 0),
            "case {case:?}: Valid must agree with the issue count, got {report}"
        );
        for issue in issues {
            // The members `SourceValidationIssue` requires, plus the rule code the
            // recorded behaviour evidences.
            assert_eq!(
                issue["Severity"].as_str(),
                Some("ERROR"),
                "case {case:?}: every issue carries a modelled Severity, got {issue}"
            );
            assert_eq!(
                issue["Code"].as_str(),
                Some("CRITICAL_ERROR"),
                "case {case:?}: every issue carries a Code, got {issue}"
            );
            assert!(
                issue["Message"]
                    .as_str()
                    .is_some_and(|text| !text.is_empty()),
                "case {case:?}: every issue says what is wrong, got {issue}"
            );
            assert!(
                issue["Line"].is_null() || issue["Line"].as_u64().is_some(),
                "case {case:?}: a reported Line is a number, got {issue}"
            );
        }
    }
}

#[tokio::test]
async fn validate_source_locates_the_missing_element_it_found() {
    let service = HealthLake::new();
    // One element at a time, so each row's report must name THAT element: the finding
    // — its message, its `Field` and its `Xpath` — is computed from the document, not a
    // fixed sentence.
    for omitted in CCD_HEADER_ELEMENTS {
        let document = ccd(&[omitted], Some(&structured_body(1)));
        let (status, report) = service
            .rest(
                "POST",
                "/validate-source?sourceFormat=CCDA",
                "application/octet-stream",
                document.into_bytes(),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {omitted:?}: a content-level outcome is HTTP 200, got {status}: {report}"
        );
        assert_eq!(
            report["Valid"].as_bool(),
            Some(false),
            "case {omitted:?}: a document missing a required element is not valid, got {report}"
        );
        let issue = &report["Issues"][0];
        let described = issue["Message"].as_str().unwrap_or_default().to_string();
        assert!(
            described.contains(omitted),
            "case {omitted:?}: the issue must name the missing element, got {described:?}"
        );
        assert_eq!(
            issue["Field"].as_str(),
            Some(*omitted),
            "case {omitted:?}: the issue's Field is the element it is about, got {issue}"
        );
        assert_eq!(
            issue["Xpath"].as_str(),
            Some(format!("/ClinicalDocument/{omitted}").as_str()),
            "case {omitted:?}: the issue's Xpath locates that element, got {issue}"
        );
        assert_eq!(
            issue["Line"].as_u64(),
            Some(1),
            "case {omitted:?}: the suite's documents are one line, so the reported Line is \
             the root element's, got {issue}"
        );
    }
}

#[tokio::test]
async fn validate_source_refuses_a_request_it_cannot_validate() {
    let service = HealthLake::new();
    let document = complete_ccd();

    // (case, query string, request body, a word the refusal must name)
    //
    // Request-level problems, unlike every content-level one above: a `sourceFormat`
    // outside the modelled enum, one the enum declares but this endpoint does not
    // validate, and the two rules the modelled `@httpPayload` member carries (it is
    // `@required`, and its target's `@length` caps it at 10 MiB).
    let oversize = "x".repeat(10 * 1024 * 1024 + 1);
    let rows: Vec<(&str, &str, String, &str)> = vec![
        (
            "a modelled format this endpoint does not validate",
            "?sourceFormat=CSV",
            document.clone(),
            "CCDA",
        ),
        (
            "a format outside the modelled enum",
            "?sourceFormat=FHIR",
            document.clone(),
            "sourceFormat",
        ),
        (
            "the validated format in the wrong case",
            "?sourceFormat=ccda",
            document.clone(),
            "sourceFormat",
        ),
        (
            "an empty sourceFormat",
            "?sourceFormat=",
            document.clone(),
            "sourceFormat",
        ),
        ("no sourceFormat at all", "", document.clone(), "sourceFormat"),
        (
            "no document at all",
            "?sourceFormat=CCDA",
            String::new(),
            "sourceFile",
        ),
        (
            "a document past the modelled payload bound",
            "?sourceFormat=CCDA",
            oversize,
            "sourceFile",
        ),
    ];

    for (case, query, body, named) in rows {
        let (status, refused) = service
            .rest(
                "POST",
                &format!("/validate-source{query}"),
                "application/octet-stream",
                body.into_bytes(),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a request-level problem is refused with HTTP 400, got {status}: \
             {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case:?}: the refusal is this service's validation error, got {refused}"
        );
        assert!(
            refused["Message"]
                .as_str()
                .is_some_and(|message| message.contains(named)),
            "case {case:?}: the refusal must name {named:?} in its modelled Message, got \
             {refused}"
        );
    }

    // And the same document under the format this endpoint DOES validate is reported,
    // not refused — so the 400s above are about the format, not the document.
    let (status, report) = service
        .rest(
            "POST",
            "/validate-source?sourceFormat=CCDA",
            "application/octet-stream",
            document.into_bytes(),
        )
        .await;
    assert_eq!(
        status, 200,
        "the validated source format is reported, got {status}: {report}"
    );
    assert_eq!(
        report["Valid"].as_bool(),
        Some(true),
        "the positive control is a valid document, got {report}"
    );
}

#[tokio::test]
async fn the_rpc_surface_is_unchanged_by_the_rest_surfaces() {
    let service = HealthLake::new();
    // An awsJson1_0 call still routes by its target…
    let (status, listed) = service.rpc("ListFHIRDatastores", &json!({})).await;
    assert_eq!(
        status, 200,
        "the RPC surface still answers its own calls, got {status}: {listed}"
    );
    // …and a request that names no operation at all is still the routing failure, not
    // one of the REST surfaces.
    let (status, refused) = service
        .rest("POST", "/", "application/x-amz-json-1.0", b"{}".to_vec())
        .await;
    assert_eq!(
        status, 400,
        "a request naming no operation keeps the protocol's own refusal, got {status}: {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "SerializationException",
        "the refusal is the protocol's, got {refused}"
    );
}

/// Every address an emulator may be reached on, as the client that is talking to it
/// sees it.
///
/// A published address is rooted at the emulator's OWN base, so the rows differ along
/// what that base can be: a router that is not served on a port, a loopback address
/// with a port, another port on that same address, an IPv6 literal, and a named host
/// with no port at all.
fn served_bases() -> [(&'static str, &'static str); 5] {
    [
        (
            "an emulator that is not served on a port",
            smithy_server_runtime::UNSERVED_URL_BASE,
        ),
        ("a loopback address and port", "https://127.0.0.1:37471"),
        (
            "another port on the same loopback address",
            "https://127.0.0.1:44223",
        ),
        ("an IPv6 loopback address and port", "https://[::1]:9297"),
        ("a named host with no port", "https://emulator.test"),
    ]
}

/// Create one data store and answer its `DatastoreId` and the whole
/// `CreateFHIRDatastore` body.
async fn created_datastore(service: &HealthLake, name: &str) -> (String, Value) {
    let (status, created) = service
        .rpc(
            "CreateFHIRDatastore",
            &json!({ "DatastoreTypeVersion": "R4", "DatastoreName": name }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {name:?}: CreateFHIRDatastore must answer 200, got {status}: {created}"
    );
    let id = created["DatastoreId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: the create reports a DatastoreId, got {created}"))
        .to_string();
    (id, created)
}

/// The `DatastoreEndpoint` of a `DatastoreProperties` (or of a response that carries it
/// at the top level).
fn published_endpoint(case: &str, body: &Value) -> String {
    body["DatastoreEndpoint"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a DatastoreEndpoint is required, got {body}"))
        .to_string()
}

/// `DatastoreEndpoint` carries a routable address, and nothing else in the API tells a
/// client where a data store's FHIR plane is — so every surface that projects it
/// publishes THIS emulator's own base, the address the client is talking to, followed
/// by the data store's own path.
#[tokio::test]
async fn every_surface_publishes_the_datastore_endpoint_on_the_address_this_emulator_is_served_on()
{
    for (case, base) in served_bases() {
        let service = HealthLake::served_at(base);
        let (id, created) = created_datastore(&service, case).await;
        let expected = format!("{base}/datastore/{id}/r4/");

        // The surfaces that project `DatastoreProperties`: the create that mints the
        // data store, the read, the listing, and the delete.
        assert_eq!(
            published_endpoint(case, &created),
            expected,
            "case {case:?}: CreateFHIRDatastore must publish the data store's endpoint on \
             this emulator's own base {base:?}, got {created}"
        );
        let properties = service.poll_until_active(&id).await;
        assert_eq!(
            published_endpoint(case, &properties),
            expected,
            "case {case:?}: DescribeFHIRDatastore must publish {expected:?}, got {properties}"
        );
        let (status, listed) = service.rpc("ListFHIRDatastores", &json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListFHIRDatastores must answer 200, got {status}: {listed}"
        );
        let entries = listed["DatastorePropertiesList"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let entry = entries
            .iter()
            .find(|entry| entry["DatastoreId"] == json!(id))
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: the listing reports {id}, got {listed}"));
        assert_eq!(
            published_endpoint(case, &entry),
            expected,
            "case {case:?}: ListFHIRDatastores must publish {expected:?}, got {entry}"
        );
        let (status, deleted) = service
            .rpc("DeleteFHIRDatastore", &json!({ "DatastoreId": id }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteFHIRDatastore must answer 200, got {status}: {deleted}"
        );
        assert_eq!(
            published_endpoint(case, &deleted),
            expected,
            "case {case:?}: DeleteFHIRDatastore must publish {expected:?}, got {deleted}"
        );

        // The authority it names is the one the client reached this emulator on, so it
        // is not the real service's host however the request was addressed.
        let authority = base
            .split_once("://")
            .map(|(_scheme, rest)| rest)
            .unwrap_or_else(|| panic!("case {case:?}: {base:?} is an absolute URL"));
        assert_eq!(
            endpoint_authority(&expected),
            authority,
            "case {case:?}: the published endpoint must name the authority this emulator is \
             reached on, got {expected:?}"
        );
        assert!(
            !endpoint_authority(&expected).ends_with("amazonaws.com"),
            "case {case:?}: the published endpoint must not name the real service's host, \
             got {expected:?}"
        );
    }
}

/// The authority of an absolute URL: everything between the scheme and the path.
fn endpoint_authority(url: &str) -> &str {
    let after_scheme = url.split_once("://").map_or(url, |(_scheme, rest)| rest);
    after_scheme.split('/').next().unwrap_or_default()
}

/// A client does with `DatastoreEndpoint` the one thing the member exists for: it sends
/// the FHIR interactions there. So the published value is an address this emulator
/// answers those interactions on — dialled whole, authority included, rather than by
/// the path alone.
#[tokio::test]
async fn a_client_that_follows_the_published_datastore_endpoint_reaches_the_fhir_plane() {
    for (case, base) in served_bases() {
        let service = HealthLake::served_at(base);
        let (id, _created) = created_datastore(&service, case).await;
        let endpoint = published_endpoint(case, &service.poll_until_active(&id).await);

        let submitted = json!({
            "resourceType": "Patient",
            "name": [{ "family": "Lovelace", "given": ["Ada"] }],
        });
        let (status, stored) = service
            .fhir_at_url("POST", &format!("{endpoint}Patient"), &submitted)
            .await;
        assert_eq!(
            status, 201,
            "case {case:?}: a FHIR create sent to the published {endpoint:?} must answer \
             201, got {status}: {stored}"
        );
        let resource_id = stored["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the create assigns an id, got {stored}"))
            .to_string();

        let (status, read) = service
            .fhir_at_url(
                "GET",
                &format!("{endpoint}Patient/{resource_id}"),
                &Value::Null,
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a FHIR read sent to the published {endpoint:?} must answer 200, \
             got {status}: {read}"
        );
        assert_eq!(
            (read["resourceType"].as_str(), read["id"].as_str()),
            (Some("Patient"), Some(resource_id.as_str())),
            "case {case:?}: the read must answer the resource the create stored, got {read}"
        );
        assert_eq!(
            read["name"], submitted["name"],
            "case {case:?}: the read must answer the submitted content, got {read}"
        );
    }
}

/// The address a client reached this emulator on is the emulator's, not the request's:
/// neither the endpoint host an SDK resolved nor the Region its credential is scoped to
/// changes the published endpoint. The Region a resource belongs to is named by its
/// ARN, which DOES follow that scope.
#[tokio::test]
async fn the_published_endpoint_follows_the_emulator_and_not_the_request_host_or_region() {
    let base = "https://127.0.0.1:37471";
    let service = HealthLake::served_at(base);

    // (case, the Host header, the Region the credential is scoped to)
    let rows: [(&str, &str, &str); 5] = [
        (
            "the endpoint host an SDK resolves",
            "healthlake.us-east-1.amazonaws.com",
            "us-east-1",
        ),
        (
            "the endpoint host of another Region",
            "healthlake.us-west-2.amazonaws.com",
            "us-west-2",
        ),
        (
            "the data-transformation endpoint host",
            "datatransformation.healthlake.eu-west-1.amazonaws.com",
            "eu-west-1",
        ),
        (
            "a dual-stack endpoint host",
            "healthlake.ap-south-1.api.aws",
            "ap-south-1",
        ),
        (
            "the literal address the emulator is reached on",
            "127.0.0.1:37471",
            "us-east-1",
        ),
    ];

    for (case, host, region) in rows {
        let (status, created) = service
            .rpc_from(
                host,
                region,
                "CreateFHIRDatastore",
                &json!({ "DatastoreTypeVersion": "R4", "DatastoreName": case }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateFHIRDatastore must answer 200, got {status}: {created}"
        );
        let id = created["DatastoreId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the create reports a DatastoreId, got {created}"))
            .to_string();
        assert_eq!(
            published_endpoint(case, &created),
            format!("{base}/datastore/{id}/r4/"),
            "case {case:?}: the published endpoint must name this emulator's base {base:?} \
             whatever host and Region the request carried, got {created}"
        );
        let expected_arn = format!("arn:aws:healthlake:{region}:000000000000:datastore/fhir/{id}");
        assert_eq!(
            created["DatastoreArn"].as_str(),
            Some(expected_arn.as_str()),
            "case {case:?}: the ARN is what names the Region the data store belongs to, \
             got {created}"
        );
    }
}
