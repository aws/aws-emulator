// SPDX-License-Identifier: CC0-1.0

//! The AWS-published parameters of the `/aws` hierarchy, end to end over the front door.
//!
//! RULE under test: a parameter name in the hierarchy AWS publishes its own parameters in
//! is not the account's — no caller can write there — so every read of Parameter Store
//! resolves such a name against the published catalog instead of the store, and reports it
//! as a `String` parameter whose value is read out of the state that OWNS the published
//! data (the Amazon EC2 image catalog: the public parameter name an image is published
//! under, and that image's id). A name neither the catalog nor the store holds is still a
//! miss, and the account's own hierarchy still reports only what the account wrote.
//!
//! Every case goes through the wire a client uses: `POST /` with
//! `X-Amz-Target: AmazonSSM.<Operation>` and an `awsJson1_1` body for Parameter Store
//! (`OPERATIONS.md`, "Wire protocol"), and an `ec2Query` form POST for the Amazon EC2 read
//! that proves the reported id names an image this account can really describe. Nothing
//! reaches into a state struct.
//!
//! The suites are tables whose rows are the WHOLE published catalog as the emulator
//! publishes it — discovered through the hierarchy read itself rather than written here —
//! so they span every namespace in it (the PCS compute-node-group AMI aliases, the Amazon
//! Linux and Windows AMI aliases), every name form in those namespaces, each page size the
//! model allows, and both halves of the namespace split.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::parameter_store::{call, error_code, ACCOUNT, REGION};

/// The hierarchy AWS publishes its own parameters in, as a client writes it.
const PUBLISHED_ROOT: &str = "/aws";

/// The namespace the published AMI aliases of AWS's services live in — the level below
/// [`PUBLISHED_ROOT`] a client reads to discover them.
const PUBLISHED_SERVICE_ROOT: &str = "/aws/service";

/// The AWS PCS namespace a compute-node-group workflow reads its recommended AMI from.
const PCS_NAMESPACE: &str = "/aws/service/pcs";

/// The leaf suffix a PCS AMI alias for 64-bit x86 carries, which is what a client filters
/// the PCS namespace by to find the image for an x86_64 node group.
const PCS_X86_LEAF: &str = "/x86_64/latest/ami-id";

/// The two other published AMI-alias namespaces, so the suites below span more than the one
/// the failing workflow reads.
pub(super) const AMAZON_LINUX_NAMESPACE: &str = "/aws/service/ami-amazon-linux-latest";

/// See [`AMAZON_LINUX_NAMESPACE`].
const WINDOWS_NAMESPACE: &str = "/aws/service/ami-windows-latest";

/// The published bounds of `GetParametersByPathMaxResults`
/// (`smithy_query(query="shape", shape="GetParametersByPathMaxResults")`: range 1..10).
const MIN_PAGE_SIZE: i64 = 1;

/// See [`MIN_PAGE_SIZE`].
const MAX_PAGE_SIZE: i64 = 10;

/// The `ParameterType` a published alias carries, from the modelled enum
/// (`smithy_query(query="shape", shape="ParameterType")`).
pub(super) const PUBLISHED_TYPE: &str = "String";

/// The `ParameterDataType` it carries (`smithy_query(query="doc",
/// shape="PutParameterRequest", member="DataType")`: "text aws:ec2:image
/// aws:ssm:integration").
pub(super) const PUBLISHED_DATA_TYPE: &str = "text";

/// The EC2 API version an ec2Query body carries.
const EC2_API_VERSION: &str = "2016-11-15";

/// A router with Parameter Store AND Amazon EC2 mounted on ONE shared engine.
///
/// EC2 is there because the published catalog is projected from ITS image state: the ids a
/// read of the `/aws` hierarchy reports are the ids of the images that service holds, and
/// the suites below check both sides of that through each service's own front door.
pub(super) fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signatures are not what these tests are about; the router still reads the SigV4
    // credential scope, which is where each call's Region comes from.
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_ssm_amazonssm::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_ec2_amazonec2::EmulatedService::new(engine),
    );
    router
}

/// One page of a hierarchy read: `(status, body)`.
async fn by_path(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "GetParametersByPath", body).await
}

/// Every parameter a read of `path` reports — recursively or at the level immediately
/// below it — paging until the read hands out no further token, the way a client's
/// paginator walks the hierarchy.
pub(super) async fn listing(
    router: &MultiServiceRouter,
    case: &str,
    path: &str,
    recursive: bool,
) -> Vec<Value> {
    let mut collected: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut body = json!({ "Path": path, "Recursive": recursive });
        if let Some(resume) = &token {
            body["NextToken"] = json!(resume);
        }
        let (status, response) = by_path(router, &body).await;
        assert_eq!(
            status, 200,
            "case {case}: expected HTTP 200 from GetParametersByPath {body}, got {status} \
             with {response}"
        );
        let page = response["Parameters"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        collected.extend(page);
        match response["NextToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
        assert!(
            collected.len() < 1_000,
            "case {case}: GetParametersByPath {path} kept handing out tokens past \
             {} parameters",
            collected.len()
        );
    }
    collected
}

/// [`listing`] with `Recursive` set: every parameter under `path`, however deep.
pub(super) async fn recursive_listing(
    router: &MultiServiceRouter,
    case: &str,
    path: &str,
) -> Vec<Value> {
    listing(router, case, path, true).await
}

/// The `Name` of each reported parameter, in the order the read reported them.
pub(super) fn names(parameters: &[Value]) -> Vec<String> {
    parameters
        .iter()
        .map(|parameter| {
            parameter["Name"]
                .as_str()
                .unwrap_or_else(|| panic!("every reported parameter carries a Name: {parameter}"))
                .to_string()
        })
        .collect()
}

/// Whether `value` is shaped like an AMI id: the form PCS's own `AmiId` member publishes
/// (`smithy_query(query="shape", shape="AmiId")` in the PCS model: pattern
/// `^ami-[a-z0-9]+$`), which is also the form EC2 mints its image ids in.
pub(super) fn is_ami_id(value: &str) -> bool {
    value.strip_prefix("ami-").is_some_and(|rest| {
        !rest.is_empty()
            && rest
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
    })
}

/// Whether the published name `name` names an IMAGE: an AMI alias, or the `image_id` leaf of
/// an image-metadata hierarchy. Every OTHER published name carries a fact published beside an
/// image — its own name, its build version, the operating system it is built on, a runtime
/// version — which is not an id.
pub(super) fn names_an_image(name: &str) -> bool {
    name.ends_with("/image_id")
        || name.ends_with("/ami-id")
        || name.starts_with("/aws/service/ami-")
}

/// The hierarchy one level above `name` (`/a/b/c` -> `/a/b`).
fn parent_path(name: &str) -> String {
    let (parent, _leaf) = name
        .rsplit_once('/')
        .unwrap_or_else(|| panic!("a published name is hierarchical: {name}"));
    parent.to_string()
}

/// The text of one XML element of an `ec2Query` body, or `None` when the body carries none —
/// as much of a reader as the Amazon EC2 assertions of this crate's published-catalog suites
/// need, in ONE place for all of them.
pub(super) fn element(body: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].to_string())
}

/// One Amazon EC2 `DescribeImages` call for `image_id`, as its status and XML body.
///
/// EC2 speaks ec2Query: an `application/x-www-form-urlencoded` POST carrying
/// `Action=<Operation>`, answered with XML, selected by the `ec2` signing name of the
/// request's SigV4 credential scope (the EC2 crate's `OPERATIONS.md`, "Wire protocol").
pub(super) async fn describe_image(router: &MultiServiceRouter, image_id: &str) -> (u16, String) {
    let body = format!(
        "Action=DescribeImages&Version={EC2_API_VERSION}&ImageId.1={image_id}"
    );
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://ec2.{REGION}.amazonaws.com/"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20240101/{REGION}/ec2/aws4_request, \
                 SignedHeaders=host;x-amz-date, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("a well-formed ec2Query request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (
        status,
        String::from_utf8_lossy(response.body()).to_string(),
    )
}

/// RULE: a recursive read of a hierarchy inside AWS's published namespace reports the
/// parameters AWS publishes under it, and reports nothing that is not under it; a path
/// inside that namespace that is no NAMESPACE of it — nothing this endpoint serves a name
/// at or under — is refused as denied rather than answered with an empty page, because the
/// reserved hierarchy holds only what AWS publishes there and no caller can write it.
#[tokio::test]
async fn every_published_namespace_resolves_under_its_own_path() {
    let router = emulator();

    /// What a row expects of its listing.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Expect {
        /// AWS publishes parameters under this path.
        Published,
        /// The path is no namespace of the reserved hierarchy, so the read is denied.
        Denied,
    }

    let rows: &[(&str, Expect)] = &[
        (PUBLISHED_ROOT, Expect::Published),
        (PUBLISHED_SERVICE_ROOT, Expect::Published),
        (PCS_NAMESPACE, Expect::Published),
        (AMAZON_LINUX_NAMESPACE, Expect::Published),
        (WINDOWS_NAMESPACE, Expect::Published),
        // Namespaces of the reserved hierarchy nothing is published under, at two levels.
        ("/aws/service/no-such-published-namespace", Expect::Denied),
        ("/aws/no-such-level", Expect::Denied),
    ];

    for (path, expect) in rows {
        let case = format!("recursive listing of {path}");
        if *expect == Expect::Denied {
            let (status, refused) =
                by_path(&router, &json!({ "Path": path, "Recursive": true })).await;
            assert_eq!(
                (status, error_code(&refused)),
                (400, "AccessDeniedException".to_string()),
                "case {case}: a path that is no namespace of the reserved hierarchy is \
                 denied, got {status} {refused}"
            );
            continue;
        }
        let reported = recursive_listing(&router, &case, path).await;
        let reported_names = names(&reported);
        assert!(
            !reported_names.is_empty(),
            "case {case}: expected AWS's published parameters under {path}, got none"
        );
        for name in &reported_names {
            assert!(
                name.starts_with(&format!("{path}/")),
                "case {case}: expected every reported name to lie under {path}, got {name:?}"
            );
        }
        // Ascending name order, which is what makes the resume cursor a position in one
        // total order rather than an offset.
        let mut sorted = reported_names.clone();
        sorted.sort();
        assert_eq!(
            reported_names, sorted,
            "case {case}: expected the names in ascending order, got {reported_names:?}"
        );
    }
}

/// RULE, for the namespace the compute-node-group workflow reads: the PCS namespace
/// publishes at least one 64-bit x86 recommended-AMI alias, and the alias resolves to an
/// AMI id — the value a client passes as the `amiId` of a compute node group.
#[tokio::test]
async fn the_pcs_namespace_publishes_an_x86_64_recommended_ami() {
    let router = emulator();
    let case = "the PCS recommended AMI aliases";
    let published = recursive_listing(&router, case, PCS_NAMESPACE).await;
    let recommended: Vec<String> = names(&published)
        .into_iter()
        .filter(|name| name.ends_with(PCS_X86_LEAF))
        .collect();
    assert!(
        !recommended.is_empty(),
        "case {case}: expected at least one name under {PCS_NAMESPACE} ending \
         {PCS_X86_LEAF}, got {:?}",
        names(&published)
    );
    for name in &recommended {
        let read = read_published(&router, name).await;
        let value = read["Value"].as_str().unwrap_or_default();
        assert!(
            is_ami_id(value),
            "case {name}: expected the alias to resolve to an AMI id, got {value:?}"
        );
    }
}

/// `GetParameter` for a published name, asserting it was served and returning the reported
/// `Parameter`.
pub(super) async fn read_published(router: &MultiServiceRouter, name: &str) -> Value {
    let (status, response) = call(router, "GetParameter", &json!({ "Name": name })).await;
    assert_eq!(
        status, 200,
        "case {name}: expected HTTP 200 from GetParameter, got {status} with {response}"
    );
    response["Parameter"].clone()
}

/// RULE: every name the published catalog holds resolves through EVERY read of Parameter
/// Store — the single read, the batch read, the hierarchy read of its own level and the
/// history read — reported as the same `String` parameter, with the account-less ARN of a
/// parameter AWS publishes rather than the account owns.
#[tokio::test]
async fn a_published_name_resolves_through_every_read() {
    let router = emulator();
    let catalog = recursive_listing(&router, "the whole published catalog", PUBLISHED_ROOT).await;
    let published = names(&catalog);
    assert!(
        published.len() > 1,
        "the catalog must publish more than one name for this table to span, got {published:?}"
    );

    for name in &published {
        let case = format!("published name {name}");
        let single = read_published(&router, name).await;
        let value = single["Value"].as_str().unwrap_or_default().to_string();
        let version = single["Version"].as_i64().unwrap_or_default();

        assert_eq!(
            single["Name"].as_str(),
            Some(name.as_str()),
            "case {case}: expected the read to echo the name, got {single}"
        );
        assert_eq!(
            single["Type"].as_str(),
            Some(PUBLISHED_TYPE),
            "case {case}: expected Type {PUBLISHED_TYPE}, got {single}"
        );
        assert_eq!(
            single["DataType"].as_str(),
            Some(PUBLISHED_DATA_TYPE),
            "case {case}: expected DataType {PUBLISHED_DATA_TYPE}, got {single}"
        );
        assert!(
            version >= 1,
            "case {case}: expected a server-assigned version of at least 1, got {single}"
        );
        if names_an_image(name) {
            assert!(
                is_ami_id(&value),
                "case {case}: expected the value to be an AMI id, got {value:?}"
            );
        } else {
            assert!(
                !value.is_empty(),
                "case {case}: expected the published fact to carry a value, got {single}"
            );
        }
        // AWS publishes these parameters itself, in every account alike, so the ARN names
        // no account: the account field is empty where an account's own parameter carries
        // this endpoint's account id.
        assert_eq!(
            single["ARN"].as_str(),
            Some(format!("arn:aws:ssm:{REGION}::parameter{name}").as_str()),
            "case {case}: expected the published-parameter ARN form, got {single}"
        );
        assert!(
            !single["ARN"].as_str().unwrap_or_default().contains(ACCOUNT),
            "case {case}: expected no account in a published parameter's ARN, got {single}"
        );

        // The batch read resolves it too, and reports it as resolved rather than invalid.
        let (status, batch) = call(
            &router,
            "GetParameters",
            &json!({ "Names": [name, "/aws/service/no-such-published-name"] }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: expected HTTP 200 from GetParameters, got {status} with {batch}"
        );
        assert_eq!(
            batch["Parameters"].as_array().map(Vec::len),
            Some(1),
            "case {case}: expected the one published name among the two asked, got {batch}"
        );
        assert_eq!(
            batch["Parameters"][0], single,
            "case {case}: expected the batch read to report what the single read did, got \
             {batch}"
        );
        assert_eq!(
            batch["InvalidParameters"].as_array().map(Vec::len),
            Some(1),
            "case {case}: expected only the unpublished name to be reported invalid, got \
             {batch}"
        );

        // The hierarchy read of the name's OWN level reports it, without recursion.
        let parent = parent_path(name);
        let at_level = names(&listing(&router, &case, &parent, false).await);
        assert!(
            at_level.contains(name),
            "case {case}: expected a one-level listing of {parent} to report it, got \
             {at_level:?}"
        );

        // The history read reports the published version of it, and only that one.
        let (status, history) =
            call(&router, "GetParameterHistory", &json!({ "Name": name })).await;
        assert_eq!(
            status, 200,
            "case {case}: expected HTTP 200 from GetParameterHistory, got {status} with \
             {history}"
        );
        let entries = history["Parameters"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            entries.len(),
            1,
            "case {case}: expected the one published version in the history, got {history}"
        );
        assert_eq!(
            entries[0]["Value"].as_str(),
            Some(value.as_str()),
            "case {case}: expected the history to carry the published value, got {history}"
        );

        // A selector naming the published version resolves to the same value; one naming a
        // version nothing published is the modelled version-not-found.
        let selected = read_published(&router, &format!("{name}:{version}")).await;
        assert_eq!(
            selected["Value"].as_str(),
            Some(value.as_str()),
            "case {case}: expected the selector {version} to report the published value, \
             got {selected}"
        );
        let (status, missing) = call(
            &router,
            "GetParameter",
            &json!({ "Name": format!("{name}:999999999") }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected a version nothing published to be refused, got {status} \
             with {missing}"
        );
        assert_eq!(
            error_code(&missing),
            "ParameterVersionNotFound",
            "case {case}: expected ParameterVersionNotFound, got {missing}"
        );
    }
}

/// RULE: a hierarchy read of the published namespace pages the way every hierarchy read
/// does — at most `MaxResults` parameters per page, resuming at the name the token names,
/// losing and repeating nothing — for every page size the model allows.
#[tokio::test]
async fn the_published_hierarchy_pages_without_loss() {
    let router = emulator();
    let whole = names(&recursive_listing(&router, "the unpaged listing", PUBLISHED_ROOT).await);
    assert!(
        whole.len() > usize::try_from(MIN_PAGE_SIZE).unwrap_or(1),
        "the catalog must publish more names than the smallest page holds, got {whole:?}"
    );

    for page_size in [MIN_PAGE_SIZE, 2, 3, MAX_PAGE_SIZE] {
        let case = format!("paging the published hierarchy at MaxResults={page_size}");
        let mut collected: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut body = json!({
                "Path": PUBLISHED_SERVICE_ROOT,
                "Recursive": true,
                "MaxResults": page_size,
            });
            if let Some(resume) = &token {
                body["NextToken"] = json!(resume);
            }
            let (status, response) = by_path(&router, &body).await;
            assert_eq!(
                status, 200,
                "case {case}: expected HTTP 200 for {body}, got {status} with {response}"
            );
            let page = names(response["Parameters"].as_array().unwrap_or(&Vec::new()));
            assert!(
                i64::try_from(page.len()).unwrap_or(i64::MAX) <= page_size,
                "case {case}: expected at most {page_size} parameters in a page, got \
                 {page:?}"
            );
            collected.extend(page);
            match response["NextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
            assert!(
                collected.len() <= whole.len(),
                "case {case}: paging reported more names than the hierarchy holds: \
                 {collected:?} against {whole:?}"
            );
        }
        let expected: Vec<String> = whole
            .iter()
            .filter(|name| name.starts_with(&format!("{PUBLISHED_SERVICE_ROOT}/")))
            .cloned()
            .collect();
        assert_eq!(
            collected, expected,
            "case {case}: expected the pages to carry the whole hierarchy in name order"
        );
    }
}

/// RULE: a published parameter is AWS's, not the account's — a write to its name is refused
/// — and reading it changes nothing a later caller sees.
#[tokio::test]
async fn a_published_parameter_is_read_only() {
    let router = emulator();
    let catalog = recursive_listing(&router, "the catalog to write into", PUBLISHED_ROOT).await;
    let published = names(&catalog);

    for name in published.iter().take(3) {
        let case = format!("writing to published name {name}");
        let before = read_published(&router, name).await;
        let (status, refused) = call(
            &router,
            "PutParameter",
            &json!({
                "Name": name,
                "Value": "ami-0000000000000000f",
                "Type": PUBLISHED_TYPE,
                "Overwrite": true,
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected a write to AWS's own hierarchy to be refused, got \
             {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ParameterPatternMismatchException",
            "case {case}: expected the reserved-name refusal, got {refused}"
        );
        let after = read_published(&router, name).await;
        assert_eq!(
            before, after,
            "case {case}: expected the published parameter to be unchanged by the refused \
             write"
        );
    }

    // Two identical reads of the hierarchy report the same thing: a read of the catalog
    // materialises nothing and consumes nothing.
    let first = by_path(&router, &json!({ "Path": PCS_NAMESPACE, "Recursive": true })).await;
    let second = by_path(&router, &json!({ "Path": PCS_NAMESPACE, "Recursive": true })).await;
    assert_eq!(
        first, second,
        "expected two identical hierarchy reads of {PCS_NAMESPACE} to report the same page"
    );
}

/// RULE (the backed outcome): the id a published name that NAMES an image resolves to names
/// an image the account can really read through Amazon EC2's own front door, and the
/// architecture the name carries is that image's architecture.
#[tokio::test]
async fn a_published_alias_resolves_to_an_image_ec2_describes() {
    let router = emulator();
    let catalog = recursive_listing(&router, "the catalog to describe", PUBLISHED_ROOT).await;
    let mut described = 0usize;

    for parameter in &catalog {
        let name = parameter["Name"].as_str().unwrap_or_default().to_string();
        if !names_an_image(&name) {
            continue;
        }
        described += 1;
        let case = format!("the image published as {name}");
        let image_id = read_published(&router, &name).await["Value"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let (status, body) = describe_image(&router, &image_id).await;
        assert_eq!(
            status, 200,
            "case {case}: expected HTTP 200 from DescribeImages for {image_id}, got \
             {status} with {body}"
        );
        assert!(
            body.contains(&format!("<imageId>{image_id}</imageId>")),
            "case {case}: expected DescribeImages to report {image_id}, got {body}"
        );
        // The architecture level of an alias names the architecture of the image it
        // resolves to, for every alias whose name carries one.
        for architecture in ["x86_64", "arm64"] {
            if name.contains(&format!("/{architecture}/")) || name.ends_with(architecture) {
                assert!(
                    body.contains(&format!("<architecture>{architecture}</architecture>")),
                    "case {case}: expected the image to be {architecture}, got {body}"
                );
            }
        }
    }

    assert!(
        described > 1,
        "the catalog must publish more than one name that names an image for this table to \
         span, got {described}"
    );
}

/// RULE: the account's own hierarchy is unaffected — it reports what the account wrote and
/// none of AWS's published parameters, and a name neither the store nor the catalog holds is
/// still the modelled miss.
#[tokio::test]
async fn the_accounts_own_hierarchy_reports_only_its_own_parameters() {
    let router = emulator();
    let mine = [("/mine/one", "first"), ("/mine/two", "second")];
    for (name, value) in mine {
        let (status, response) = call(
            &router,
            "PutParameter",
            &json!({ "Name": name, "Value": value, "Type": PUBLISHED_TYPE }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name}: PutParameter must succeed, got {status} with {response}"
        );
    }

    let rows: &[(&str, &[&str])] = &[
        ("/", &["/mine/one", "/mine/two"]),
        ("/mine", &["/mine/one", "/mine/two"]),
    ];
    for (path, expected) in rows {
        let case = format!("the account's own listing of {path}");
        let reported = names(&recursive_listing(&router, &case, path).await);
        assert_eq!(
            reported,
            expected
                .iter()
                .map(|name| (*name).to_string())
                .collect::<Vec<String>>(),
            "case {case}: expected only the account's own parameters"
        );
    }

    // A name in AWS's hierarchy that AWS publishes nothing under is still a miss, and so is
    // one in the account's.
    for name in ["/aws/service/no-such-published-name", "/mine/three"] {
        let (status, response) = call(&router, "GetParameter", &json!({ "Name": name })).await;
        assert_eq!(
            status, 400,
            "case {name}: expected a name nothing holds to be a miss, got {status} with \
             {response}"
        );
        assert_eq!(
            error_code(&response),
            "ParameterNotFound",
            "case {name}: expected ParameterNotFound, got {response}"
        );
    }
}
