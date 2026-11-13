// SPDX-License-Identifier: CC0-1.0

//! The S3 Control access point lifecycle, driven end to end over HTTP.
//!
//! Every assertion here goes through the same front door a client uses: a
//! buffered `http::Request` dispatched by the multi-service router, which
//! deserializes it per the Smithy model, runs the handler, and serializes the
//! restXml response. Nothing calls a handler directly, so a passing row here is
//! a claim about the wire, not about a Rust function.
//!
//! Each test drives a TABLE of inputs rather than one recorded call: the rows
//! differ along what the behaviour generalises over — whether the access point
//! is VPC-restricted, whether it carries a public-access block, the bucket it
//! fronts, the length and shape of its name, the page size a list asks for, and
//! the bytes of the policy put on it.
//!
//! The router, the request builder, the XML escaping and the response reader
//! below are `pub(crate)` because the sibling suites in `tests/all/` stand an
//! emulator up the same way rather than each writing its own harness.

use std::collections::BTreeSet;
use std::sync::Arc;

use smithy_server_runtime::MultiServiceRouter;

/// The five XML predefined entities, as (escaped form, the character it stands
/// for). The emulator's restXml serializer escapes exactly these and writes
/// neither CDATA nor numeric character references, so reading and writing them
/// is the whole of the text encoding this suite meets.
///
/// Spelled out here rather than taken from an XML crate deliberately: a new
/// dev-dependency puts this service's crate into `Cargo.lock`, which every
/// other service's landing also touches, and the squash merge then conflicts.
const XML_ENTITIES: [(&str, char); 5] = [
    ("&amp;", '&'),
    ("&lt;", '<'),
    ("&gt;", '>'),
    ("&quot;", '"'),
    ("&apos;", '\''),
];

/// Escape a value for an XML text node — what the emulator's own parser
/// resolves on the way in, for the request bodies this suite builds by hand.
pub(crate) fn xml_escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match XML_ENTITIES.iter().find(|(_, plain)| *plain == character) {
            Some((entity, _)) => escaped.push_str(entity),
            None => escaped.push(character),
        }
    }
    escaped
}

/// Resolve the escaped forms in an element's text, the inverse of
/// [`xml_escape`].
pub(crate) fn xml_unescape(text: &str) -> String {
    let mut plain = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(ampersand) = rest.find('&') {
        plain.push_str(&rest[..ampersand]);
        let reference = &rest[ampersand..];
        match XML_ENTITIES
            .iter()
            .find(|(entity, _)| reference.starts_with(entity))
        {
            Some((entity, character)) => {
                plain.push(*character);
                rest = &reference[entity.len()..];
            }
            // Not an escaped form this producer emits: the ampersand is part of
            // the value.
            None => {
                plain.push('&');
                rest = &reference['&'.len_utf8()..];
            }
        }
    }
    plain.push_str(rest);
    plain
}

/// The region every request in this suite is signed for. The emulator reads it
/// from the request's SigV4 credential scope, so it is also the region expected
/// in every ARN below.
pub(crate) const REGION: &str = "us-west-2";

/// The account every request below is the caller's own: the single account this
/// emulator's credentials present (`::emulator_server::EMULATOR_ACCOUNT_ID`),
/// which is the only one S3 Control authorizes a request for.
pub(crate) const ACCOUNT: &str = ::emulator_server::EMULATOR_ACCOUNT_ID;

/// A second account — one the caller holds no credentials for — used to pin that
/// a request addressed in another account's control plane is refused rather than
/// served.
pub(crate) const OTHER_ACCOUNT: &str = "210987654321";

/// The XML namespace this service's model declares.
pub(crate) const XMLNS: &str = "http://awss3control.amazonaws.com/doc/2018-08-20/";

/// The status this service answers a success that carries NO document with, as
/// the service states it
/// ([`::service_com_amazonaws_s3control_awss3controlservicev20180820::state::NO_CONTENT_SUCCESS_STATUS`]).
///
/// Every operation whose modelled output is `smithy.api#Unit` answers one: the
/// deletes, and the puts that report nothing back.
///
/// `pub(crate)` because every suite in `tests/all/` that drives such a call
/// asserts the same status rather than each spelling a number.
pub(crate) const NO_CONTENT: u16 =
    ::service_com_amazonaws_s3control_awss3controlservicev20180820::state::NO_CONTENT_SUCCESS_STATUS;

/// The answer S3 Control gives a request addressed in an account the caller
/// holds no credentials for: the account a request names says whose control
/// plane it is for, and it is authorized ahead of the resource the request
/// names, so the answer is the same for every operation and whether or not that
/// resource exists.
pub(crate) const FOREIGN_ACCOUNT_STATUS: u16 = 403;
pub(crate) const FOREIGN_ACCOUNT_CODE: &str = "AccessDenied";
pub(crate) const FOREIGN_ACCOUNT_MESSAGE: &str = "Access Denied";

/// Assert `reply` is that refusal, naming `account` — the account the request
/// asked for — in the member a caller reads it from.
pub(crate) fn assert_foreign_account_refused(case: &str, account: &str, reply: &Reply) {
    assert_eq!(
        (reply.status, reply.code().as_str()),
        (FOREIGN_ACCOUNT_STATUS, FOREIGN_ACCOUNT_CODE),
        "case {case:?}: expected a request for another account to be refused \
         {FOREIGN_ACCOUNT_STATUS} {FOREIGN_ACCOUNT_CODE}, got {} with body {}",
        reply.status,
        reply.body
    );
    assert_eq!(
        reply.text("Message").as_deref(),
        Some(FOREIGN_ACCOUNT_MESSAGE),
        "case {case:?}: expected the fixed refusal message, got body {}",
        reply.body
    );
    assert_eq!(
        reply.text("AccountId").as_deref(),
        Some(account),
        "case {case:?}: expected the refusal to name the account the request asked for, \
         got body {}",
        reply.body
    );
}

/// How many characters of the access point name the alias can carry: the
/// modelled `Alias` maximum (63) less the `-s3alias` tail, the 26-character
/// service-assigned segment, and the hyphen before it.
const ALIAS_NAME_ROOM: usize = 63 - "-s3alias".len() - 26 - 1;

/// One response: the status, the headers and the body, with readers for the XML
/// elements.
pub(crate) struct Reply {
    pub(crate) status: u16,
    /// The response headers as `(lower-case name, value)` pairs, in arrival
    /// order — the correlation headers an error answer carries are read from
    /// here.
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) body: String,
}

impl Reply {
    /// The value of the first `name` header, compared case-insensitively as HTTP
    /// compares a header name.
    pub(crate) fn header(&self, name: &str) -> Option<String> {
        self.headers
            .iter()
            .find(|(sent, _)| sent.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.clone())
    }
    /// The text of the first `element` in the body, or `None` when the body
    /// carries none.
    pub(crate) fn text(&self, element: &str) -> Option<String> {
        self.texts(element).into_iter().next()
    }

    /// The text of every `<element>…</element>` in the body, in document order.
    ///
    /// The emulator writes a flat document: no attributes on these elements, no
    /// CDATA, no layout whitespace between tags. An element's text is therefore
    /// exactly the bytes between its open and close tags, unescaped. An element
    /// written in the empty form (`<Element/>`) carries no text and is reported
    /// as absent, which is how an absent `NextToken` is read below.
    pub(crate) fn texts(&self, element: &str) -> Vec<String> {
        let open = format!("<{element}>");
        let close = format!("</{element}>");
        let mut found = Vec::new();
        let mut rest = self.body.as_str();
        while let Some(start) = rest.find(&open) {
            let after_open = &rest[start + open.len()..];
            let Some(end) = after_open.find(&close) else {
                panic!(
                    "the emulator answered <{element}> with no closing tag: {}",
                    self.body
                )
            };
            found.push(xml_unescape(&after_open[..end]));
            rest = &after_open[end + close.len()..];
        }
        found
    }

    /// The S3-style error code (`<Code>`) of a failure response.
    pub(crate) fn code(&self) -> String {
        self.text("Code")
            .unwrap_or_else(|| format!("<no Code element> in {}", self.body))
    }
}

/// One emulator: a router with this service mounted on a single shared engine,
/// exactly as `core/router` assembles it.
///
/// Only THIS service is mounted — one suite below pins that S3 Control does not
/// claim Amazon S3's own path space — so the buckets an access point is created
/// over are recorded in Amazon S3's store through that store's own API
/// ([`Self::provision_bucket`]), which is the store this service resolves a
/// bucket in. The sibling [`crate::object_store::ObjectStore`] harness mounts
/// both services and drives the same prerequisite over the wire.
pub(crate) struct Emulator {
    pub(crate) router: MultiServiceRouter,
    /// The one shared engine both stores live in, kept so a suite can record the
    /// bucket an access point is created over.
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    pub(crate) fn start() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These requests carry an unsigned `Authorization` header, which is
        // what makes them route to the `s3` signing name; signature
        // verification is the transport's concern, not this suite's.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_s3control_awss3controlservicev20180820::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        Self { router, engine }
    }

    /// Make sure Amazon S3's store holds a bucket named `bucket` in [`REGION`].
    ///
    /// The prerequisite of every access point created below: an access point is a
    /// resource OF a bucket, so S3 Control creates one only over a bucket that
    /// exists. A name the store already holds is left exactly as it is, so a
    /// suite whose rows share a bucket may ask for it once per row.
    pub(crate) fn provision_bucket(&self, case: &str, bucket: &str) {
        self.provision_bucket_in(case, bucket, REGION);
    }

    /// [`Self::provision_bucket`], in `region`.
    ///
    /// A Multi-Region Access Point associates buckets in SEVERAL Regions, and
    /// the Region a member is reported in is the bucket's own, so a suite that
    /// drives one records each member bucket where it means it to live.
    pub(crate) fn provision_bucket_in(&self, case: &str, bucket: &str, region: &str) {
        let state = ::service_com_amazonaws_s3_amazons3::state::service_state(&*self.engine)
            .unwrap_or_else(|| panic!("case {case:?}: the object store stands up"));
        state.lock().create_bucket(bucket, region);
    }

    pub(crate) async fn call(
        &self,
        method: &str,
        uri: &str,
        account_id: &str,
        body: &str,
    ) -> Reply {
        call_in_region(&self.router, REGION, method, uri, account_id, body).await
    }

    async fn create(&self, account_id: &str, case: &CreateCase) -> Reply {
        self.provision_bucket(case.case, case.bucket);
        self.call(
            "PUT",
            &format!("/v20180820/accesspoint/{}", case.name),
            account_id,
            &case.request_body(),
        )
        .await
    }

    async fn get(&self, account_id: &str, name: &str) -> Reply {
        self.call(
            "GET",
            &format!("/v20180820/accesspoint/{name}"),
            account_id,
            "",
        )
        .await
    }

    async fn delete(&self, account_id: &str, name: &str) -> Reply {
        self.call(
            "DELETE",
            &format!("/v20180820/accesspoint/{name}"),
            account_id,
            "",
        )
        .await
    }

    async fn put_policy(&self, account_id: &str, name: &str, policy: &str) -> Reply {
        let body = format!(
            "<PutAccessPointPolicyRequest xmlns=\"{XMLNS}\"><Policy>{}</Policy>\
             </PutAccessPointPolicyRequest>",
            xml_escape(policy)
        );
        self.call(
            "PUT",
            &format!("/v20180820/accesspoint/{name}/policy"),
            account_id,
            &body,
        )
        .await
    }

    async fn get_policy(&self, account_id: &str, name: &str) -> Reply {
        self.call(
            "GET",
            &format!("/v20180820/accesspoint/{name}/policy"),
            account_id,
            "",
        )
        .await
    }

    async fn list(&self, account_id: &str, query: &str) -> Reply {
        let uri = if query.is_empty() {
            "/v20180820/accesspoint".to_string()
        } else {
            format!("/v20180820/accesspoint?{query}")
        };
        self.call("GET", &uri, account_id, "").await
    }
}

/// One `CreateAccessPoint` request: the members the model lets a caller vary.
struct CreateCase {
    case: &'static str,
    name: &'static str,
    bucket: &'static str,
    /// `Some` makes the access point VPC-restricted, which is what decides
    /// `NetworkOrigin`.
    vpc_id: Option<&'static str>,
    /// `BlockPublicAcls`, `IgnorePublicAcls`, `BlockPublicPolicy`,
    /// `RestrictPublicBuckets` — in the order
    /// `PublicAccessBlockConfiguration` declares them.
    public_access_block: Option<[bool; 4]>,
    /// The account owning the bucket, required by the model only for a
    /// cross-account access point.
    bucket_account_id: Option<&'static str>,
}

impl CreateCase {
    const fn new(case: &'static str, name: &'static str, bucket: &'static str) -> Self {
        Self {
            case,
            name,
            bucket,
            vpc_id: None,
            public_access_block: None,
            bucket_account_id: None,
        }
    }

    fn request_body(&self) -> String {
        let mut xml = format!("<CreateAccessPointRequest xmlns=\"{XMLNS}\">");
        xml.push_str(&format!("<Bucket>{}</Bucket>", xml_escape(self.bucket)));
        if let Some(vpc_id) = self.vpc_id {
            xml.push_str(&format!(
                "<VpcConfiguration><VpcId>{}</VpcId></VpcConfiguration>",
                xml_escape(vpc_id)
            ));
        }
        if let Some([acls, ignore, policy, restrict]) = self.public_access_block {
            xml.push_str(&format!(
                "<PublicAccessBlockConfiguration>\
                 <BlockPublicAcls>{acls}</BlockPublicAcls>\
                 <IgnorePublicAcls>{ignore}</IgnorePublicAcls>\
                 <BlockPublicPolicy>{policy}</BlockPublicPolicy>\
                 <RestrictPublicBuckets>{restrict}</RestrictPublicBuckets>\
                 </PublicAccessBlockConfiguration>"
            ));
        }
        if let Some(bucket_account_id) = self.bucket_account_id {
            xml.push_str(&format!(
                "<BucketAccountId>{}</BucketAccountId>",
                xml_escape(bucket_account_id)
            ));
        }
        xml.push_str("</CreateAccessPointRequest>");
        xml
    }

    /// `VPC` when the create call restricted the access point to a VPC,
    /// otherwise `Internet` — the rule `AccessPoint$NetworkOrigin` states.
    fn expected_network_origin(&self) -> &'static str {
        if self.vpc_id.is_some() {
            "VPC"
        } else {
            "Internet"
        }
    }
}

/// Send one request through the router, signed for `region`.
///
/// Taken out of [`Emulator::call`] so a suite that varies the signing region —
/// the region every ARN this service reports is in — reaches the same request
/// builder rather than writing a second one.
pub(crate) async fn call_in_region(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    account_id: &str,
    body: &str,
) -> Reply {
    let request = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header(
            "host",
            format!("{account_id}.s3-control.{region}.amazonaws.com"),
        )
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{region}/s3/aws4_request, \
                 SignedHeaders=host, Signature=abc"
            ),
        )
        .header("x-amz-account-id", account_id)
        .header("content-type", "application/xml")
        .body(::bytes::Bytes::from(body.to_owned()))
        .expect("build the request");
    let response = router.dispatch(request).await;
    Reply {
        status: response.status().as_u16(),
        headers: response
            .headers()
            .iter()
            .map(|(name, value)| {
                (
                    name.as_str().to_ascii_lowercase(),
                    String::from_utf8_lossy(value.as_bytes()).to_string(),
                )
            })
            .collect(),
        body: String::from_utf8(response.body().to_vec()).expect("a UTF-8 response body"),
    }
}

/// The ARN an access point of this name must report: the `accesspoint/<name>`
/// resource in the `s3` ARN namespace, in the signing region, under the
/// emulator's canonical account.
fn expected_arn(name: &str) -> String {
    format!(
        "arn:aws:s3:{REGION}:{}:accesspoint/{name}",
        ::emulator_server::emulator_account_id()
    )
}

/// The create cases every lifecycle assertion is run over. A name is never
/// merely renamed between rows: each row differs in something the behaviour
/// under test generalises over.
fn create_cases() -> Vec<CreateCase> {
    vec![
        CreateCase::new("internet, no optional members", "reports-ap", "reports"),
        CreateCase {
            vpc_id: Some("vpc-0a1b2c3d4e5f67890"),
            ..CreateCase::new("vpc-restricted", "vpc-ap", "reports")
        },
        CreateCase {
            public_access_block: Some([true, true, true, true]),
            ..CreateCase::new("public access fully blocked", "locked-ap", "finance")
        },
        CreateCase {
            public_access_block: Some([true, false, true, false]),
            ..CreateCase::new("public access partly blocked", "half-locked-ap", "finance")
        },
        CreateCase {
            vpc_id: Some("vpc-00000000000000001"),
            public_access_block: Some([false, false, false, true]),
            bucket_account_id: Some(OTHER_ACCOUNT),
            ..CreateCase::new("cross-account bucket, vpc and block", "partner-ap", "partner")
        },
        // The shortest name the model's `len 3..255` admits.
        CreateCase::new("minimum-length name", "abc", "reports"),
        CreateCase::new("digits and hyphens in the name", "ap-7-9", "reports"),
        // Longer than the alias can carry, which is what makes the alias
        // truncate rather than overrun the modelled 63-character maximum.
        CreateCase::new(
            "name longer than the alias can carry",
            "a-very-long-access-point-name-for-alias-truncation",
            "reports",
        ),
    ]
}

#[tokio::test]
async fn create_then_get_reports_the_stored_access_point_for_every_shape_of_create() {
    let emulator = Emulator::start();
    let mut aliases = BTreeSet::new();

    for case in create_cases() {
        let created = emulator.create(ACCOUNT, &case).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: expected CreateAccessPoint to answer 200, got {} with body {}",
            case.case, created.status, created.body
        );

        let arn = created.text("AccessPointArn").unwrap_or_default();
        assert_eq!(
            arn,
            expected_arn(case.name),
            "case {:?}: expected the computed access point ARN, got {arn:?}",
            case.case
        );

        let alias = created.text("Alias").unwrap_or_default();
        let name_head: String = case.name.to_lowercase().chars().take(ALIAS_NAME_ROOM).collect();
        assert!(
            alias.starts_with(&name_head) && alias.ends_with("-s3alias"),
            "case {:?}: expected an alias of <name>-<suffix>-s3alias, got {alias:?}",
            case.case
        );
        assert!(
            alias.len() <= 63,
            "case {:?}: expected an alias within the modelled 63-character maximum, got {} characters ({alias:?})",
            case.case,
            alias.len()
        );
        assert!(
            aliases.insert(alias.clone()),
            "case {:?}: expected a distinct alias per access point, got a repeat of {alias:?}",
            case.case
        );

        let read = emulator.get(ACCOUNT, case.name).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: expected GetAccessPoint to answer 200, got {} with body {}",
            case.case, read.status, read.body
        );
        assert_eq!(
            read.text("Name").as_deref(),
            Some(case.name),
            "case {:?}: expected the stored name, got body {}",
            case.case,
            read.body
        );
        assert_eq!(
            read.text("Bucket").as_deref(),
            Some(case.bucket),
            "case {:?}: expected the stored bucket, got body {}",
            case.case,
            read.body
        );
        assert_eq!(
            read.text("NetworkOrigin").as_deref(),
            Some(case.expected_network_origin()),
            "case {:?}: expected NetworkOrigin derived from VpcConfiguration, got body {}",
            case.case,
            read.body
        );
        assert_eq!(
            read.text("VpcId").as_deref(),
            case.vpc_id,
            "case {:?}: expected the stored VpcConfiguration, got body {}",
            case.case,
            read.body
        );
        assert_eq!(
            read.text("BucketAccountId").as_deref(),
            Some(case.bucket_account_id.unwrap_or(ACCOUNT)),
            "case {:?}: expected the account that owns the bucket — the one the create \
             named, or this account when it named none — got body {}",
            case.case,
            read.body
        );
        assert_eq!(
            read.text("AccessPointArn").as_deref(),
            Some(arn.as_str()),
            "case {:?}: expected GetAccessPoint to report the ARN CreateAccessPoint returned, got body {}",
            case.case,
            read.body
        );
        assert_eq!(
            read.text("Alias").as_deref(),
            Some(alias.as_str()),
            "case {:?}: expected GetAccessPoint to report the alias CreateAccessPoint returned, got body {}",
            case.case,
            read.body
        );

        let flags = ["BlockPublicAcls", "IgnorePublicAcls", "BlockPublicPolicy", "RestrictPublicBuckets"];
        // Public access is blocked by default, so a create that supplied no
        // configuration leaves all four settings on; one that supplied a
        // configuration leaves exactly it.
        let expected_block = case
            .public_access_block
            .unwrap_or([true, true, true, true]);
        for (flag, expected) in flags.iter().zip(expected_block) {
            assert_eq!(
                read.text(flag).as_deref(),
                Some(if expected { "true" } else { "false" }),
                "case {:?}: expected the effective {flag}, got body {}",
                case.case,
                read.body
            );
        }

        let creation_date = read.text("CreationDate").unwrap_or_default();
        assert!(
            creation_date.contains('T') && creation_date.ends_with('Z'),
            "case {:?}: expected an ISO-8601 CreationDate, got {creation_date:?}",
            case.case
        );

        // A read may not change what a later reader sees: the recorded creation
        // date is the one from the create, on every later call.
        let read_again = emulator.get(ACCOUNT, case.name).await;
        assert_eq!(
            read_again.text("CreationDate"),
            Some(creation_date),
            "case {:?}: expected CreationDate to be the instant of the create on every read, got body {}",
            case.case,
            read_again.body
        );
    }
}

#[tokio::test]
async fn a_repeat_create_is_a_conflict_and_leaves_the_stored_access_point_alone() {
    let emulator = Emulator::start();

    for case in create_cases() {
        assert_eq!(
            emulator.create(ACCOUNT, &case).await.status,
            200,
            "case {:?}: expected the first create to succeed",
            case.case
        );
        let first = emulator.get(ACCOUNT, case.name).await;

        // The repeat differs from the first create in its other members too, so
        // a conflict that silently overwrote the record would be visible below.
        let repeat = CreateCase {
            vpc_id: Some("vpc-ffffffffffffffff0"),
            ..CreateCase::new(case.case, case.name, "a-different-bucket")
        };
        let conflict = emulator.create(ACCOUNT, &repeat).await;
        assert_eq!(
            conflict.status, 409,
            "case {:?}: expected a repeat create to answer 409, got {} with body {}",
            case.case, conflict.status, conflict.body
        );
        assert_eq!(
            conflict.code(),
            "AccessPointAlreadyOwnedByYou",
            "case {:?}: expected the conflict error code, got body {}",
            case.case,
            conflict.body
        );

        let after = emulator.get(ACCOUNT, case.name).await;
        assert_eq!(
            after.text("Bucket"),
            first.text("Bucket"),
            "case {:?}: expected the refused create to leave the stored bucket alone, got body {}",
            case.case,
            after.body
        );
        assert_eq!(
            after.text("CreationDate"),
            first.text("CreationDate"),
            "case {:?}: expected the refused create to leave the stored creation date alone, got body {}",
            case.case,
            after.body
        );
    }
}

#[tokio::test]
async fn a_policy_round_trips_byte_for_byte_and_the_last_put_wins() {
    let emulator = Emulator::start();
    // Created with every public-access setting spelled OFF, so this suite stays
    // about the bytes of the document: a public document put on an access point
    // whose `BlockPublicPolicy` is on is refused rather than stored, which the
    // sibling suite pins.
    let case = CreateCase {
        public_access_block: Some([false, false, false, false]),
        ..CreateCase::new("policy round trip", "policy-ap", "reports")
    };
    assert_eq!(emulator.create(ACCOUNT, &case).await.status, 200);

    // Documents differing in whitespace, escaping, nesting and alphabet: every
    // one must come back exactly as it was put.
    let policies = [
        ("empty statement list", r#"{"Version":"2012-10-17","Statement":[]}"#),
        (
            "one statement",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"AWS":"*"},"Action":"s3:GetObject","Resource":"arn:aws:s3:us-west-2:000000000000:accesspoint/policy-ap/object/*"}]}"#,
        ),
        (
            "indented, with newlines and tabs",
            "{\n\t\"Version\": \"2012-10-17\",\n\t\"Statement\": [\n\t\t{\n\t\t\t\"Effect\": \"Deny\"\n\t\t}\n\t]\n}",
        ),
        (
            "XML-significant characters in a value",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Deny","Condition":{"StringLike":{"s3:prefix":"a<b&c>d'e\"f"}}}]}"#,
        ),
        (
            "non-ASCII characters in a value",
            r#"{"Version":"2012-10-17","Statement":[{"Sid":"rapport-été-☃","Effect":"Allow"}]}"#,
        ),
        (
            // Nested through the elements the policy language defines — a
            // condition block, its operators, their keys and their value lists
            // — because a member the language does not define is refused
            // whatever it nests.
            "deeply nested document",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Deny","Condition":{"StringLike":{"s3:prefix":["a","b"]},"Bool":{"aws:SecureTransport":[false]}}}]}"#,
        ),
    ];

    for (label, policy) in policies {
        let put = emulator.put_policy(ACCOUNT, case.name, policy).await;
        assert_eq!(
            put.status, NO_CONTENT,
            "case {label:?}: expected PutAccessPointPolicy to answer {NO_CONTENT}, got {} with \
             body {}",
            put.status, put.body
        );

        let read = emulator.get_policy(ACCOUNT, case.name).await;
        assert_eq!(
            read.status, 200,
            "case {label:?}: expected GetAccessPointPolicy to answer 200, got {} with body {}",
            read.status, read.body
        );
        assert_eq!(
            read.text("Policy").as_deref(),
            Some(policy),
            "case {label:?}: expected the policy back byte for byte, got body {}",
            read.body
        );
    }

    // The table ran in order, so the policy now stored is the last row's: a put
    // replaces the document rather than appending to it.
    let last = policies.last().expect("a policy row").1;
    let read = emulator.get_policy(ACCOUNT, case.name).await;
    assert_eq!(
        read.text("Policy").as_deref(),
        Some(last),
        "expected the last policy put to be the stored one, got body {}",
        read.body
    );
}

#[tokio::test]
async fn list_access_points_filters_by_bucket_and_pages_through_every_access_point() {
    let emulator = Emulator::start();
    let cases = create_cases();
    for case in &cases {
        assert_eq!(
            emulator.create(ACCOUNT, case).await.status,
            200,
            "case {:?}: expected the create to succeed",
            case.case
        );
    }
    // The same create addressed in another account's control plane is refused
    // before anything is stored, so no listing of this account's can hold it.
    assert_foreign_account_refused(
        "CreateAccessPoint in another account",
        OTHER_ACCOUNT,
        &emulator
            .create(OTHER_ACCOUNT, &CreateCase::new("other account", "reports-ap", "reports"))
            .await,
    );

    let all: BTreeSet<String> = cases.iter().map(|case| case.name.to_string()).collect();

    let listed = emulator.list(ACCOUNT, "").await;
    assert_eq!(
        listed.status, 200,
        "expected ListAccessPoints to answer 200, got {} with body {}",
        listed.status, listed.body
    );
    let names: Vec<String> = listed.texts("Name");
    assert_eq!(
        names.iter().cloned().collect::<BTreeSet<_>>(),
        all,
        "expected every access point of the account, got {names:?}"
    );
    assert!(
        listed.text("NextToken").is_none(),
        "expected no continuation token when the page holds everything, got body {}",
        listed.body
    );
    for case in &cases {
        assert!(
            listed.body.contains(&expected_arn(case.name)),
            "case {:?}: expected the listed entry to carry the access point ARN, got body {}",
            case.case,
            listed.body
        );
    }

    // A read must not change what a later reader sees.
    let again = emulator.list(ACCOUNT, "").await;
    assert_eq!(
        again.texts("Name"),
        names,
        "expected a repeated list to answer identically, got body {}",
        again.body
    );

    // A listing of another account's access points is refused rather than
    // answered, so one account's listing never enumerates another's.
    assert_foreign_account_refused(
        "ListAccessPoints in another account",
        OTHER_ACCOUNT,
        &emulator.list(OTHER_ACCOUNT, "").await,
    );

    // Filtering by bucket: one row per bucket the table uses. A bucket no
    // access point fronts is NOT a row here — it carries no collection to page
    // through, and the refusal it answers is pinned beside this suite.
    for bucket in ["reports", "finance", "partner"] {
        let expected: BTreeSet<String> = cases
            .iter()
            .filter(|case| case.bucket == bucket)
            .map(|case| case.name.to_string())
            .collect();
        let filtered = emulator.list(ACCOUNT, &format!("bucket={bucket}")).await;
        assert_eq!(
            filtered.status, 200,
            "case {bucket:?}: expected the filtered list to answer 200, got {} with body {}",
            filtered.status, filtered.body
        );
        assert_eq!(
            filtered.texts("Name").into_iter().collect::<BTreeSet<_>>(),
            expected,
            "case {bucket:?}: expected only the access points fronting that bucket, got body {}",
            filtered.body
        );
    }

    // Paging: every page size from 1 to one past the number of access points,
    // plus the model's maximum, must walk the whole set exactly once.
    for page_size in [1usize, 2, 3, cases.len(), cases.len() + 1, 1000] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match &token {
                None => format!("maxResults={page_size}"),
                Some(token) => format!("maxResults={page_size}&nextToken={token}"),
            };
            let page = emulator.list(ACCOUNT, &query).await;
            assert_eq!(
                page.status, 200,
                "case page size {page_size}: expected 200, got {} with body {}",
                page.status, page.body
            );
            let page_names = page.texts("Name");
            assert!(
                page_names.len() <= page_size,
                "case page size {page_size}: expected at most that many entries, got {page_names:?}"
            );
            assert!(
                !page_names.is_empty(),
                "case page size {page_size}: expected a non-empty page while a token was issued, got body {}",
                page.body
            );

            // The same token must answer the same page: a token is a position in
            // a scan, not a cursor the service consumes.
            if let Some(token) = &token {
                let replayed = emulator
                    .list(ACCOUNT, &format!("maxResults={page_size}&nextToken={token}"))
                    .await;
                assert_eq!(
                    replayed.texts("Name"),
                    page_names,
                    "case page size {page_size}: expected a token to answer the same page twice, got body {}",
                    replayed.body
                );
            }

            seen.extend(page_names);
            pages += 1;
            assert!(
                pages <= cases.len() + 1,
                "case page size {page_size}: expected paging to finish, saw {pages} pages"
            );
            match page.text("NextToken") {
                Some(next) => token = Some(next),
                None => break,
            }
        }
        assert_eq!(
            seen.len(),
            all.len(),
            "case page size {page_size}: expected every access point exactly once, got {seen:?}"
        );
        assert_eq!(
            seen.iter().cloned().collect::<BTreeSet<_>>(),
            all,
            "case page size {page_size}: expected the pages to cover the whole set, got {seen:?}"
        );
    }

    // A page size and a bucket filter compose: paging a filtered scan walks only
    // the filtered access points.
    let finance: BTreeSet<String> = cases
        .iter()
        .filter(|case| case.bucket == "finance")
        .map(|case| case.name.to_string())
        .collect();
    let first = emulator.list(ACCOUNT, "bucket=finance&maxResults=1").await;
    assert_eq!(
        first.texts("Name").len(),
        1,
        "expected one entry on a one-entry page, got body {}",
        first.body
    );
    let token = first
        .text("NextToken")
        .expect("expected a continuation token while finance access points remain");
    let second = emulator
        .list(ACCOUNT, &format!("bucket=finance&maxResults=1&nextToken={token}"))
        .await;
    let paged: BTreeSet<String> = first
        .texts("Name")
        .into_iter()
        .chain(second.texts("Name"))
        .collect();
    assert_eq!(
        paged, finance,
        "expected paging a filtered list to cover exactly the filtered access points, got {paged:?}"
    );
}

#[tokio::test]
async fn delete_removes_the_access_point_and_its_policy() {
    let emulator = Emulator::start();

    for case in create_cases() {
        assert_eq!(emulator.create(ACCOUNT, &case).await.status, 200);
        assert_eq!(
            emulator
                .put_policy(ACCOUNT, case.name, r#"{"Version":"2012-10-17","Statement":[]}"#)
                .await
                .status,
            NO_CONTENT,
            "case {:?}: expected the policy put to succeed",
            case.case
        );

        let deleted = emulator.delete(ACCOUNT, case.name).await;
        assert_eq!(
            deleted.status, NO_CONTENT,
            "case {:?}: expected DeleteAccessPoint to answer {NO_CONTENT}, got {} with body {}",
            case.case, deleted.status, deleted.body
        );

        let read = emulator.get(ACCOUNT, case.name).await;
        assert_eq!(
            (read.status, read.code().as_str()),
            (404, "NoSuchAccessPoint"),
            "case {:?}: expected the deleted access point to be gone, got body {}",
            case.case,
            read.body
        );
        let policy = emulator.get_policy(ACCOUNT, case.name).await;
        assert_eq!(
            (policy.status, policy.code().as_str()),
            (404, "NoSuchAccessPoint"),
            "case {:?}: expected the policy to go with the access point, got body {}",
            case.case,
            policy.body
        );
        let listed = emulator.list(ACCOUNT, "").await;
        assert!(
            !listed.texts("Name").iter().any(|name| name == case.name),
            "case {:?}: expected the deleted access point to leave the list, got body {}",
            case.case,
            listed.body
        );
        let deleted_again = emulator.delete(ACCOUNT, case.name).await;
        assert_eq!(
            (deleted_again.status, deleted_again.code().as_str()),
            (404, "NoSuchAccessPoint"),
            "case {:?}: expected deleting it twice to be not-found, got body {}",
            case.case,
            deleted_again.body
        );
    }
}

#[tokio::test]
async fn every_call_against_an_access_point_that_is_not_there_is_not_found() {
    let emulator = Emulator::start();
    let present = CreateCase::new("present", "present-ap", "reports");
    assert_eq!(emulator.create(ACCOUNT, &present).await.status, 200);

    // One row per read or write that names an access point, each against a name
    // the account does not own.
    let absent = "absent-ap";
    let rows: Vec<(&str, Reply)> = vec![
        ("GetAccessPoint", emulator.get(ACCOUNT, absent).await),
        ("DeleteAccessPoint", emulator.delete(ACCOUNT, absent).await),
        (
            "GetAccessPointPolicy",
            emulator.get_policy(ACCOUNT, absent).await,
        ),
        (
            "PutAccessPointPolicy",
            emulator
                .put_policy(ACCOUNT, absent, r#"{"Version":"2012-10-17","Statement":[]}"#)
                .await,
        ),
    ];
    for (label, reply) in rows {
        assert_eq!(
            (reply.status, reply.code().as_str()),
            (404, "NoSuchAccessPoint"),
            "case {label:?}: expected 404 NoSuchAccessPoint, got {} with body {}",
            reply.status,
            reply.body
        );
    }

    // The same reads addressed in another account's control plane are refused
    // ahead of the lookup, so the answer does not say whether the name is there:
    // an access point this account DOES own is the same refusal as one it does
    // not.
    for (label, reply) in [
        (
            "GetAccessPoint in another account, name present",
            emulator.get(OTHER_ACCOUNT, present.name).await,
        ),
        (
            "GetAccessPoint in another account, name absent",
            emulator.get(OTHER_ACCOUNT, absent).await,
        ),
        (
            "DeleteAccessPoint in another account",
            emulator.delete(OTHER_ACCOUNT, present.name).await,
        ),
    ] {
        assert_foreign_account_refused(label, OTHER_ACCOUNT, &reply);
    }

    // An access point with no policy is a different failure from an access point
    // that does not exist.
    let no_policy = emulator.get_policy(ACCOUNT, present.name).await;
    assert_eq!(
        (no_policy.status, no_policy.code().as_str()),
        (404, "NoSuchAccessPointPolicy"),
        "expected 404 NoSuchAccessPointPolicy for an access point carrying no policy, got {} with body {}",
        no_policy.status,
        no_policy.body
    );
}

#[tokio::test]
async fn a_request_the_service_cannot_serve_faithfully_is_refused_rather_than_answered() {
    let emulator = Emulator::start();
    let case = CreateCase::new("refusals", "refusal-ap", "reports");
    assert_eq!(emulator.create(ACCOUNT, &case).await.status, 200);

    // A policy that is not a JSON document at all is malformed, not a value to
    // store.
    for (label, policy) in [
        ("not JSON at all", "this is not a policy"),
        ("truncated JSON", r#"{"Version":"2012-10-17""#),
        ("empty document", ""),
    ] {
        let put = emulator.put_policy(ACCOUNT, case.name, policy).await;
        assert_eq!(
            (put.status, put.code().as_str()),
            (400, "MalformedPolicy"),
            "case {label:?}: expected 400 MalformedPolicy, got {} with body {}",
            put.status,
            put.body
        );
    }

    // A continuation token this service never issued cannot resume a scan.
    for (label, token) in [
        ("not the token encoding", "not-a-token"),
        ("valid encoding of invalid bytes", "ffff"),
    ] {
        let listed = emulator.list(ACCOUNT, &format!("nextToken={token}")).await;
        assert_eq!(
            listed.status, 400,
            "case {label:?}: expected a foreign continuation token to be refused, got {} with body {}",
            listed.status,
            listed.body
        );
    }

    // A page size outside the modelled `@range(min: 0, max: 1000)` is refused
    // rather than widened. A negative one is refused at the boundary, before
    // the handler; the handler refuses it too if it ever arrives.
    for (label, max_results) in [("negative", "-1"), ("past the maximum", "1001")] {
        let listed = emulator
            .list(ACCOUNT, &format!("maxResults={max_results}"))
            .await;
        assert_eq!(
            listed.status, 400,
            "case {label:?}: expected a page size outside the modelled range to be refused, got {} with body {}",
            listed.status,
            listed.body
        );
    }

    // `Scope` is modelled for directory-bucket access points, which this
    // emulator does not have: it answers 501 rather than dropping the member.
    emulator.provision_bucket("a scoped create", "reports");
    let scoped = emulator
        .call(
            "PUT",
            "/v20180820/accesspoint/scoped-ap",
            ACCOUNT,
            &format!(
                "<CreateAccessPointRequest xmlns=\"{XMLNS}\"><Bucket>reports</Bucket>\
                 <Scope><Prefixes><Prefix>logs/</Prefix></Prefixes></Scope>\
                 </CreateAccessPointRequest>"
            ),
        )
        .await;
    assert_eq!(
        (scoped.status, scoped.code().as_str()),
        (501, "NotImplemented"),
        "expected a scoped create to answer 501, got {} with body {}",
        scoped.status,
        scoped.body
    );
    let scoped_read = emulator.get(ACCOUNT, "scoped-ap").await;
    assert_eq!(
        scoped_read.status, 404,
        "expected a refused create to store nothing, got {} with body {}",
        scoped_read.status,
        scoped_read.body
    );

    // S3 Control shares the `s3` signing name with Amazon S3, so it claims only
    // its own `/v20180820` path space: a request signed as `s3` but addressed to
    // S3's `/{Bucket}` space is not this service's, and the router finds no
    // service for it rather than this one answering for S3.
    for (label, uri) in [
        ("an S3 bucket listing", "/some-bucket?list-type=2"),
        ("an S3 object read", "/some-bucket/some/key.txt"),
        ("the root path", "/"),
    ] {
        let foreign = emulator.call("GET", uri, ACCOUNT, "").await;
        assert_eq!(
            foreign.status, 404,
            "case {label:?}: expected this service not to claim the request, got {} with body {}",
            foreign.status, foreign.body
        );
        assert!(
            foreign.body.contains("UnknownService"),
            "case {label:?}: expected the router's no-claimant answer, got body {}",
            foreign.body
        );
    }
}

/// A policy document of exactly `bytes` bytes: one statement whose `Sid` is
/// padded to the length asked for, so a document written to probe the SIZE bound
/// is still a document in the policy language (the grammar is read before the
/// bound, and a member the language does not define is refused whatever its
/// size).
fn policy_of_length(bytes: usize) -> String {
    let envelope = r#"{"Version":"2012-10-17","Statement":[{"Sid":""}]}"#.len();
    assert!(bytes >= envelope, "a policy document cannot be that short");
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Sid\":\"{}\"}}]}}",
        "a".repeat(bytes - envelope)
    )
}

#[tokio::test]
async fn the_retained_policy_document_is_bounded() {
    let emulator = Emulator::start();
    let case = CreateCase::new("policy size", "policy-size-ap", "reports");
    assert_eq!(emulator.create(ACCOUNT, &case).await.status, 200);

    let maximum = service_com_amazonaws_s3control_awss3controlservicev20180820::state::MAX_POLICY_BYTES;

    // At the bound, and one byte under it, the document is stored and reported
    // back unchanged.
    for bytes in [maximum - 1, maximum] {
        let policy = policy_of_length(bytes);
        let put = emulator.put_policy(ACCOUNT, case.name, &policy).await;
        assert_eq!(
            put.status, NO_CONTENT,
            "case {bytes} bytes: expected a document within the bound to be stored, got {} with body {}",
            put.status, put.body
        );
        let read = emulator.get_policy(ACCOUNT, case.name).await;
        assert_eq!(
            read.text("Policy").as_deref(),
            Some(policy.as_str()),
            "case {bytes} bytes: expected the document back byte for byte"
        );
    }

    // One byte past it, and far past it, the document is refused and the stored
    // one is left alone.
    let stored = policy_of_length(maximum);
    for bytes in [maximum + 1, maximum * 4] {
        let policy = policy_of_length(bytes);
        let put = emulator.put_policy(ACCOUNT, case.name, &policy).await;
        assert_eq!(
            (put.status, put.code().as_str()),
            (400, "MalformedPolicy"),
            "case {bytes} bytes: expected a document past the bound to be refused, got {} with body {}",
            put.status,
            put.body
        );
        let read = emulator.get_policy(ACCOUNT, case.name).await;
        assert_eq!(
            read.text("Policy").as_deref(),
            Some(stored.as_str()),
            "case {bytes} bytes: expected the refused put to leave the stored policy alone"
        );
    }
}

#[tokio::test]
async fn the_number_of_stored_access_points_is_bounded_and_a_delete_releases_the_bound() {
    let emulator = Emulator::start();
    let quota =
        service_com_amazonaws_s3control_awss3controlservicev20180820::state::ACCESS_POINT_QUOTA;
    emulator.provision_bucket("the bound", "bulk");

    // Fill the store to its bound. The names differ only in their index, which
    // is the point: the key is request-derived, so a caller could otherwise mint
    // records without end.
    for index in 0..quota {
        let name = format!("bulk-ap-{index}");
        let body = format!("<CreateAccessPointRequest xmlns=\"{XMLNS}\"><Bucket>bulk</Bucket></CreateAccessPointRequest>");
        let created = emulator
            .call("PUT", &format!("/v20180820/accesspoint/{name}"), ACCOUNT, &body)
            .await;
        assert_eq!(
            created.status, 200,
            "case access point {index}: expected a create within the bound to succeed, got {} with body {}",
            created.status, created.body
        );
    }

    // Past the bound a create is refused, because the bound is on this
    // process's store.
    let refused = emulator
        .create(ACCOUNT, &CreateCase::new("same account", "over-the-bound-ap", "bulk"))
        .await;
    assert_eq!(
        (refused.status, refused.code().as_str()),
        (400, "TooManyAccessPoints"),
        "expected a create past the bound to be refused, got {} with body {}",
        refused.status,
        refused.body
    );
    // A create addressed in another account never reaches the bound at all: the
    // account it names is authorized first.
    assert_foreign_account_refused(
        "CreateAccessPoint in another account past the bound",
        OTHER_ACCOUNT,
        &emulator
            .create(OTHER_ACCOUNT, &CreateCase::new("another account", "over-the-bound-ap", "bulk"))
            .await,
    );

    // Deleting a record releases the bound, so the store cannot wedge.
    assert_eq!(
        emulator.delete(ACCOUNT, "bulk-ap-0").await.status,
        NO_CONTENT
    );
    let after = emulator
        .create(ACCOUNT, &CreateCase::new("after a delete", "over-the-bound-ap", "bulk"))
        .await;
    assert_eq!(
        after.status, 200,
        "expected a create to succeed once a delete released the bound, got {} with body {}",
        after.status, after.body
    );
}
