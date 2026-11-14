// SPDX-License-Identifier: CC0-1.0

//! The Amazon SimpleDB 2009-04-15 AWS Query API on the ASSEMBLED router: the `sdb`
//! endpoint serves it beside the modelled v2 export operations.
//!
//! The two APIs share one endpoint prefix and one SigV4 signing name (`sdb`), so which of
//! them serves a request is decided inside the one service that claims the key — the v2
//! operations by their modelled `POST /v2/*` routes, the 2009-04-15 API by the `Action`
//! field of a form-encoded request on `"/"`. That resolution is only observable on the
//! WHOLE router: a suite that mounts one service cannot show that no other service claims
//! the root-path request first, and here the `sdb` key is resolved against every
//! registered service.
//!
//! What is asserted: a SimpleDB client's `CreateDomain` is served (HTTP 200, the
//! 2009-04-15 envelope), the domain it creates is the domain the v2 `StartDomainExport`
//! resolves, and the item a `PutAttributes` writes is read back by `GetAttributes` — each
//! driven both as a signed request and as one addressed by endpoint host alone. The
//! per-action semantics (every refusal, every form of value, the paging) are the owning
//! crate's own suite, `services/service-com-amazonaws-simpledbv2-simpledbv2/tests/all/
//! simpledb_query_api.rs`; what this suite adds is the assembled-router claim.
//!
//! Assembling the whole router costs stack (see `boot_serve.rs`), so it is built — and the
//! requests driven — on a sized thread.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};
use serde_json::{json, Value};

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The signing name and endpoint prefix both APIs on this endpoint are addressed with.
const SIGNING_NAME: &str = "sdb";

/// The Region a signed request below is addressed in.
const REGION: &str = "us-west-2";

/// The API version a 2009-04-15 request names.
const API_VERSION: &str = "2009-04-15";

/// The namespace every document of that API carries.
const SDB_NAMESPACE: &str = "http://sdb.amazonaws.com/doc/2009-04-15/";

/// The bucket the exports below write to.
const BUCKET: &str = "assembled-sdb-bucket";

/// How a request addresses the emulator.
#[derive(Debug, Clone, Copy)]
enum Addressing {
    /// Signed with the `sdb` signing name, as an AWS SDK request is.
    Signed,
    /// Unsigned, addressing the endpoint host — the signal the router falls back to when a
    /// request carries no SigV4 credential scope.
    HostOnly,
}

/// The assembled emulator — every service on one engine — and the runtime its requests are
/// driven on.
struct Emulator {
    router: smithy_server_runtime::MultiServiceRouter,
    runtime: tokio::runtime::Runtime,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
        let mut router = build_router(engine);
        // These rows exercise routing and handlers, not SigV4; the credential scope still
        // has to route, and still carries the Region the handlers read.
        router.set_request_authentication(false);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("a current-thread runtime");
        Self { router, runtime }
    }

    /// One 2009-04-15 request: `(http status, response body)`.
    ///
    /// Every parameter value this suite sends is unreserved ASCII (an action name, a
    /// domain name, an attribute name or value made of letters and `-`), so the form body
    /// is the pairs joined as they are; the owning crate's suite is where values needing
    /// percent-encoding are driven, through an encoder.
    fn query(&self, addressing: Addressing, parameters: &[(&str, &str)]) -> (u16, String) {
        for (name, value) in parameters {
            assert!(
                [name, value].iter().all(|text| text
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"-_.~".contains(&byte))),
                "every parameter this suite sends is unreserved ASCII: {name}={value}"
            );
        }
        let body = parameters
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<String>>()
            .join("&");
        self.send(
            addressing,
            "/",
            "application/x-www-form-urlencoded",
            body,
        )
    }

    /// One v2 export request: `(http status, parsed body)`.
    fn v2(&self, operation: &str, body: &Value) -> (u16, Value) {
        let (status, text) = self.send(
            Addressing::Signed,
            &format!("/v2/{operation}"),
            "application/json",
            serde_json::to_string(body).expect("the request body serializes"),
        );
        (status, serde_json::from_str(&text).unwrap_or(Value::Null))
    }

    fn send(
        &self,
        addressing: Addressing,
        path: &str,
        content_type: &str,
        body: String,
    ) -> (u16, String) {
        let host = match addressing {
            Addressing::Signed => format!("{SIGNING_NAME}.{REGION}.amazonaws.com"),
            // The endpoint this API publishes for its own default Region, whose first
            // label is the only routing signal an unsigned request leaves.
            Addressing::HostOnly => format!("{SIGNING_NAME}.amazonaws.com"),
        };
        let mut request = http::Request::builder()
            .method("POST")
            .uri(format!("https://{host}{path}"))
            .header("host", host.as_str())
            .header("content-type", content_type);
        if matches!(addressing, Addressing::Signed) {
            request = request.header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 \
                     Credential=AKIDEXAMPLE/20260101/{REGION}/{SIGNING_NAME}/aws4_request, \
                     SignedHeaders=host, Signature=not-verified"
                ),
            );
        }
        let request = request
            .body(bytes::Bytes::from(body))
            .expect("a well-formed request");
        let response = self.runtime.block_on(self.router.dispatch(request));
        let status = response.status().as_u16();
        (status, String::from_utf8_lossy(response.body()).to_string())
    }
}

/// One 2009-04-15 call that must be served, returning the document it answered with.
fn served(
    emulator: &Emulator,
    addressing: Addressing,
    action: &str,
    parameters: &[(&str, &str)],
) -> String {
    let mut form: Vec<(&str, &str)> = vec![("Action", action), ("Version", API_VERSION)];
    form.extend_from_slice(parameters);
    let (status, body) = emulator.query(addressing, &form);
    assert_eq!(
        status, 200,
        "case {action} addressed {addressing:?}: expected HTTP 200 from the assembled \
         router, got {status} with {body}"
    );
    assert!(
        body.contains(&format!("<{action}Response xmlns=\"{SDB_NAMESPACE}\">")),
        "case {action} addressed {addressing:?}: expected the 2009-04-15 envelope, got {body}"
    );
    body
}

/// A member of a v2 JSON answer.
fn member(body: &Value, name: &str) -> Option<String> {
    body.get(name).and_then(Value::as_str).map(str::to_string)
}

/// Both APIs of this endpoint are served by the assembled router, and they share one
/// store: a domain created on the 2009-04-15 wire is the domain the v2 export resolves,
/// the item written there is read back from there, and deleting the domain there takes it
/// away from the export again.
#[test]
fn the_sdb_endpoint_serves_both_its_apis_on_the_assembled_router() {
    std::thread::Builder::new()
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let emulator = Emulator::new();

            for (addressing, domain) in [
                (Addressing::Signed, "assembled-signed"),
                (Addressing::HostOnly, "assembled-host-only"),
            ] {
                // Nothing holds the domain yet, so the v2 export refuses it.
                let (status, refused) = emulator.v2(
                    "StartDomainExport",
                    &json!({ "domainName": domain, "s3Bucket": BUCKET }),
                );
                assert_eq!(
                    status, 400,
                    "case {domain}: an uncreated domain must be refused, got {status} with \
                     {refused:?}"
                );

                // The 2009-04-15 wire creates it, and writes an item into it.
                served(&emulator, addressing, "CreateDomain", &[("DomainName", domain)]);
                served(
                    &emulator,
                    addressing,
                    "PutAttributes",
                    &[
                        ("DomainName", domain),
                        ("ItemName", "item-1"),
                        ("Attribute.0.Name", "colour"),
                        ("Attribute.0.Value", "blue"),
                    ],
                );
                let read_back = served(
                    &emulator,
                    addressing,
                    "GetAttributes",
                    &[("DomainName", domain), ("ItemName", "item-1")],
                );
                assert!(
                    read_back
                        .contains("<Attribute><Name>colour</Name><Value>blue</Value></Attribute>"),
                    "case {domain}: GetAttributes must report what PutAttributes wrote, got \
                     {read_back}"
                );

                // And the v2 export now resolves the very domain that was created.
                let (status, started) = emulator.v2(
                    "StartDomainExport",
                    &json!({ "domainName": domain, "s3Bucket": BUCKET }),
                );
                assert_eq!(
                    status, 200,
                    "case {domain}: the created domain must be exportable, got {status} with \
                     {started:?}"
                );
                let export_arn = member(&started, "exportArn")
                    .unwrap_or_else(|| panic!("case {domain}: no exportArn in {started:?}"));
                let (status, fetched) =
                    emulator.v2("GetExport", &json!({ "exportArn": export_arn }));
                assert_eq!(
                    (status, member(&fetched, "domainName").as_deref()),
                    (200, Some(domain)),
                    "case {domain}: GetExport must report the created domain, got {fetched:?}"
                );

                // The query wire enumerates it too, and deleting it there takes it away
                // from the v2 export again.
                let listed = served(&emulator, addressing, "ListDomains", &[]);
                assert!(
                    listed.contains(&format!("<DomainName>{domain}</DomainName>")),
                    "case {domain}: ListDomains must report the created domain, got {listed}"
                );
                served(&emulator, addressing, "DeleteDomain", &[("DomainName", domain)]);
                let (status, refused) = emulator.v2(
                    "StartDomainExport",
                    &json!({ "domainName": domain, "s3Bucket": BUCKET }),
                );
                assert_eq!(
                    status, 400,
                    "case {domain}: a deleted domain must be refused again, got {status} with \
                     {refused:?}"
                );
            }
        })
        .expect("a sized thread")
        .join()
        .expect("the assembled-router suite runs to completion");
}
