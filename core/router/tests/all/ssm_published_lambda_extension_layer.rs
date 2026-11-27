// SPDX-License-Identifier: CC0-1.0

//! The AWS-PUBLISHED public parameter that carries the AWS Parameters and Secrets Lambda
//! Extension layer ARN, driven over the two services that decide it: Systems Manager serves
//! the read of AWS's own `/aws/service` hierarchy, and Lambda publishes the layer version the
//! name resolves to.
//!
//! RULE under test: a public parameter of AWS's published `/aws/service` hierarchy is served
//! read-only to every caller in every Region, and it resolves to an identifier of a resource
//! the service that publishes it really serves — so the extension's parameter resolves to a
//! layer version ARN of the Region the read was made in, that same service's own by-ARN read
//! answers for, and a function's `Layers` mounts.
//!
//! Only a cross-service suite can pin this: the name is served by one service's read path and
//! the value it resolves to is another service's resource, so the two can only be seen to
//! agree where both are mounted over one engine.
//!
//! The rows span what the rule generalises over: each architecture level the hierarchy
//! publishes the extension under (the short and the long spelling of the 64-bit x86 one, and
//! the 64-bit Arm one, which is a build and a layer of its own), each read of Parameter Store
//! that can reach a published name (the single read, the batch read and the hierarchy read),
//! and two Regions, because a layer is a regional resource and the ARN a read reports carries
//! the Region it was read in. The published VERSION is not a row: AWS republishes the
//! extension per Region, so what is asserted of it is that the ARN names a version this
//! endpoint serves, not which number it is.

use std::collections::BTreeSet;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The account every ARN of this emulator carries, the execution role a create names and the
// deployment package it uploads, written ONCE in the suite that owns them.
use crate::lambda_signing_name_routing::{ACCOUNT, ROLE, ZIP_FILE};

/// The hierarchy of AWS's published namespace the extension's layer ARN is advertised under
/// (AWS Secrets Manager User Guide, "Use AWS Secrets Manager secrets in AWS Lambda
/// functions").
const PUBLISHED_PATH: &str = "/aws/service/aws-parameters-and-secrets-lambda-extension";

/// The level of that hierarchy naming the newest published version of a build.
const LATEST_LEAF: &str = "latest";

/// The bare layer names the extension's two builds are published under.
const X86_LAYER: &str = "AWS-Parameters-and-Secrets-Lambda-Extension";
const ARM64_LAYER: &str = "AWS-Parameters-and-Secrets-Lambda-Extension-Arm64";

/// The `ParameterType` and `ParameterDataType` AWS's published parameters are reported as:
/// the modelled `String` type and the default data type (`smithy_query(query="doc",
/// shape="Parameter", member="DataType")` — "The default is text").
const PUBLISHED_TYPE: &str = "String";
const PUBLISHED_DATA_TYPE: &str = "text";

/// The two Regions every row is read in: a layer is a regional resource, so the ARN a read
/// reports has to be of the Region the read was made in.
const REGIONS: &[&str] = &["us-east-1", "eu-west-1"];

/// The runtime and handler the function of each row declares, and how long one invocation
/// would be given. Nothing here invokes; the function exists to mount the layer.
const RUNTIME: &str = "python3.12";
const HANDLER: &str = "index.handler";

/// One architecture level of the published hierarchy.
struct Row {
    /// What makes this row different from the others.
    case: &'static str,
    /// The architecture level the name carries.
    level: &'static str,
    /// The bare layer name the level's build is published under.
    layer: &'static str,
    /// The instruction set architecture that build declares, as the model's `Architecture`
    /// enum spells it (`smithy_query(query="shape", shape="Architecture")`).
    architecture: &'static str,
}

/// Every architecture level the extension is published under, and the build each addresses.
const ROWS: &[Row] = &[
    Row {
        case: "the short spelling of the 64-bit x86 level",
        level: "x86",
        layer: X86_LAYER,
        architecture: "x86_64",
    },
    Row {
        case: "the enum's spelling of the 64-bit x86 level",
        level: "x86_64",
        layer: X86_LAYER,
        architecture: "x86_64",
    },
    Row {
        case: "the 64-bit Arm level, a build of its own",
        level: "arm64",
        layer: ARM64_LAYER,
        architecture: "arm64",
    },
];

impl Row {
    /// The public parameter name this row is read by.
    fn parameter(&self) -> String {
        format!("{PUBLISHED_PATH}/{}/{LATEST_LEAF}", self.level)
    }
}

/// Both services over ONE shared engine, as `core/router/src/lib.rs` mounts them.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Nothing here signs: the `Authorization` header carries only the credential scope, which
    // is where each service reads the Region and the caller's access key id.
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_ssm_amazonssm::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `signing_name` and `region`.
fn authorization(region: &str, signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTPUBLISHED/20260101/{region}/{signing_name}/\
         aws4_request, SignedHeaders=host, Signature=0000"
    )
}

/// The wire error code of an answer, as the short shape name a client reads (`""` on a
/// success).
fn error_code(headers: &http::HeaderMap, body: &Value) -> String {
    let raw = body
        .get("__type")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            headers
                .get("x-amzn-errortype")
                .and_then(|value| value.to_str().ok())
                .map(str::to_string)
        })
        .unwrap_or_default();
    raw.split('#').next_back().unwrap_or(&raw).to_string()
}

/// One Systems Manager call on its `awsJson1_1` wire, answered with the status, the wire error
/// code and the JSON body.
async fn ssm(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, String, Value) {
    let host = format!("ssm.{region}.amazonaws.com");
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{host}/"))
        .header("host", &host)
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("AmazonSSM.{operation}"))
        .header("authorization", authorization(region, "ssm"))
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed awsJson1_1 request");
    answered(router, request).await
}

/// One AWS Lambda call on its `restJson1` wire, answered the same way.
async fn lambda(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, String, Value) {
    let host = format!("lambda.{region}.amazonaws.com");
    let payload = body.map_or_else(Bytes::new, |document| {
        Bytes::from(serde_json::to_vec(document).expect("the request body serializes"))
    });
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{uri}"))
        .header("host", &host)
        .header("content-type", "application/json")
        .header("authorization", authorization(region, "lambda"))
        .body(payload)
        .expect("a well-formed restJson1 request");
    answered(router, request).await
}

/// Dispatch one request and read the answer every helper above reports.
async fn answered(
    router: &MultiServiceRouter,
    request: http::Request<Bytes>,
) -> (u16, String, Value) {
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    let raw = response.into_body();
    let parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    let code = error_code(&headers, &parsed);
    (status, code, parsed)
}

/// The `Name`s a hierarchy or batch read reported, as a set.
fn names(parameters: &Value, case: &str) -> BTreeSet<String> {
    parameters
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a list of parameters, got {parameters}"))
        .iter()
        .map(|parameter| {
            parameter["Name"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: every reported parameter carries a Name: {parameter}")
                })
                .to_string()
        })
        .collect()
}

/// The layer version `arn` names, as `(the bare layer name, the version number)` — or the
/// reason it is no layer version ARN of `region`.
fn layer_version_named(region: &str, arn: &str) -> Result<(String, i64), String> {
    let prefix = format!("arn:aws:lambda:{region}:{ACCOUNT}:layer:");
    let resource = arn
        .strip_prefix(&prefix)
        .ok_or_else(|| format!("expected an ARN starting {prefix:?}"))?;
    let (name, version) = resource
        .rsplit_once(':')
        .ok_or_else(|| "expected the version appended to the layer ARN".to_string())?;
    let version: i64 = version
        .parse()
        .map_err(|_| format!("expected a version number, got {version:?}"))?;
    if version < 1 {
        return Err(format!("expected a version of at least 1, got {version}"));
    }
    Ok((name.to_string(), version))
}

/// RULE: the published parameter of each architecture level is served in every Region and
/// resolves to the ARN of a layer version Lambda itself serves there — the by-ARN read answers
/// for it, and a function mounts it without the account having published a layer of its own.
#[tokio::test]
async fn the_published_extension_parameter_resolves_to_a_layer_version_lambda_serves() {
    let router = emulator();

    for region in REGIONS {
        for (index, row) in ROWS.iter().enumerate() {
            let name = row.parameter();
            let case = format!("{} in {region}", row.case);

            let (status, code, read) =
                ssm(&router, region, "GetParameter", &json!({ "Name": name })).await;
            assert_eq!(
                (status, code.as_str()),
                (200, ""),
                "case {case}: expected the published name to be served, got {status} {code:?}: \
                 {read}"
            );
            let parameter = &read["Parameter"];
            assert_eq!(
                (
                    parameter["Name"].as_str(),
                    parameter["Type"].as_str(),
                    parameter["DataType"].as_str()
                ),
                (
                    Some(name.as_str()),
                    Some(PUBLISHED_TYPE),
                    Some(PUBLISHED_DATA_TYPE)
                ),
                "case {case}: expected the name echoed as a {PUBLISHED_TYPE} parameter \
                 carrying {PUBLISHED_DATA_TYPE}, got {read}"
            );
            let value = parameter["Value"].as_str().unwrap_or_default().to_string();
            let (named_layer, version) = layer_version_named(region, &value)
                .unwrap_or_else(|why| panic!("case {case}: {why}, got {value:?}"));
            assert_eq!(
                named_layer, row.layer,
                "case {case}: expected the value to name the layer {:?}, got {value:?}",
                row.layer
            );

            // The service that PUBLISHES the layer answers for that very ARN: the parameter
            // advertises no version Lambda does not serve.
            let (status, code, held) = lambda(
                &router,
                region,
                "GET",
                &format!("/2018-10-31/layers?find=LayerVersion&Arn={value}"),
                None,
            )
            .await;
            assert_eq!(
                (status, code.as_str()),
                (200, ""),
                "case {case}: expected Lambda's by-ARN read to serve {value:?}, got {status} \
                 {code:?}: {held}"
            );
            assert_eq!(
                (
                    held["LayerVersionArn"].as_str(),
                    held["Version"].as_i64(),
                    held["CompatibleArchitectures"].as_array().map(|list| {
                        list.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect::<Vec<String>>()
                    }),
                ),
                (
                    Some(value.as_str()),
                    Some(version),
                    Some(vec![row.architecture.to_string()])
                ),
                "case {case}: expected the version's own ARN, number and architecture, got \
                 {held}"
            );

            // And a function MOUNTS it: the ARN a client resolved by name is a layer version
            // `Layers` accepts, reported back on the function's own configuration.
            let function = format!("xpub-extension-{region}-{index}");
            let (status, code, created) = lambda(
                &router,
                region,
                "POST",
                "/2015-03-31/functions",
                Some(&json!({
                    "FunctionName": function,
                    "Role": ROLE,
                    "Runtime": RUNTIME,
                    "Handler": HANDLER,
                    "Architectures": [row.architecture],
                    "Code": { "ZipFile": ZIP_FILE },
                    "Layers": [value],
                })),
            )
            .await;
            assert_eq!(
                (status, code.as_str()),
                (201, ""),
                "case {case}: expected the create to mount the published layer, got {status} \
                 {code:?}: {created}"
            );
            assert_eq!(
                created["Layers"][0]["Arn"].as_str(),
                Some(value.as_str()),
                "case {case}: expected the created function to carry the mounted layer, got \
                 {created}"
            );

            let (status, code, configuration) = lambda(
                &router,
                region,
                "GET",
                &format!("/2015-03-31/functions/{function}/configuration"),
                None,
            )
            .await;
            assert_eq!(
                (status, code.as_str()),
                (200, ""),
                "case {case}: expected the function's configuration to be read back, got \
                 {status} {code:?}: {configuration}"
            );
            assert_eq!(
                configuration["Layers"][0]["Arn"].as_str(),
                Some(value.as_str()),
                "case {case}: expected the read-back configuration to carry the mounted \
                 layer, got {configuration}"
            );
        }
    }
}

/// RULE: a published name resolves through every read of Parameter Store that can reach it —
/// the batch read reports it beside the names it could not resolve, and the hierarchy read
/// reports exactly the names published under the path it was given.
#[tokio::test]
async fn every_read_of_parameter_store_reaches_the_published_hierarchy() {
    let router = emulator();
    let region = REGIONS[0];
    let published: BTreeSet<String> = ROWS.iter().map(Row::parameter).collect();
    // A name of the same hierarchy under an architecture level nothing is published for.
    let unpublished = format!("{PUBLISHED_PATH}/sparc64/{LATEST_LEAF}");

    let mut asked: Vec<String> = published.iter().cloned().collect();
    asked.push(unpublished.clone());
    let (status, code, batch) =
        ssm(&router, region, "GetParameters", &json!({ "Names": asked })).await;
    assert_eq!(
        (status, code.as_str()),
        (200, ""),
        "case \"the batch read\": expected HTTP 200, got {status} {code:?}: {batch}"
    );
    assert_eq!(
        names(&batch["Parameters"], "the batch read"),
        published,
        "case \"the batch read\": expected every published name resolved, got {batch}"
    );
    assert_eq!(
        batch["InvalidParameters"]
            .as_array()
            .map(|list| list.iter().filter_map(Value::as_str).collect::<Vec<&str>>()),
        Some(vec![unpublished.as_str()]),
        "case \"the batch read\": expected only the unpublished name reported invalid, got \
         {batch}"
    );

    let (status, code, recursive) = ssm(
        &router,
        region,
        "GetParametersByPath",
        &json!({ "Path": PUBLISHED_PATH, "Recursive": true }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (200, ""),
        "case \"the recursive hierarchy read\": expected HTTP 200, got {status} {code:?}: \
         {recursive}"
    );
    assert_eq!(
        names(&recursive["Parameters"], "the recursive hierarchy read"),
        published,
        "case \"the recursive hierarchy read\": expected exactly the published names, got \
         {recursive}"
    );

    for row in ROWS {
        let level = format!("{PUBLISHED_PATH}/{}", row.level);
        let (status, code, listed) = ssm(
            &router,
            region,
            "GetParametersByPath",
            &json!({ "Path": level }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (200, ""),
            "case {:?}: expected HTTP 200 from the level's own read, got {status} {code:?}: \
             {listed}",
            row.case
        );
        assert_eq!(
            names(&listed["Parameters"], row.case),
            BTreeSet::from([row.parameter()]),
            "case {:?}: expected the one name published at {level}, got {listed}",
            row.case
        );
    }
}

/// RULE: only the names AWS publishes are served — a name of the same hierarchy that nothing
/// is published under is reported absent, and a layer version ARN of a layer nothing published
/// is served by neither the by-ARN read nor a function's `Layers`.
#[tokio::test]
async fn nothing_unpublished_is_served_under_the_hierarchy() {
    let router = emulator();
    let region = REGIONS[0];

    let unpublished = format!("{PUBLISHED_PATH}/sparc64/{LATEST_LEAF}");
    let (status, code, refused) = ssm(
        &router,
        region,
        "GetParameter",
        &json!({ "Name": unpublished }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ParameterNotFound"),
        "case \"an architecture level nothing is published under\": expected the modelled \
         not-found, got {status} {code:?}: {refused}"
    );

    let absent = format!("arn:aws:lambda:{region}:{ACCOUNT}:layer:{X86_LAYER}-NoSuchBuild:1");
    let (status, code, answered) = lambda(
        &router,
        region,
        "GET",
        &format!("/2018-10-31/layers?find=LayerVersion&Arn={absent}"),
        None,
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (404, "ResourceNotFoundException"),
        "case \"a layer version nothing published\": expected the modelled 404 from the \
         by-ARN read, got {status} {code:?}: {answered}"
    );

    let (status, code, answered) = lambda(
        &router,
        region,
        "POST",
        "/2015-03-31/functions",
        Some(&json!({
            "FunctionName": "xpub-extension-absent",
            "Role": ROLE,
            "Runtime": RUNTIME,
            "Handler": HANDLER,
            "Code": { "ZipFile": ZIP_FILE },
            "Layers": [absent],
        })),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "InvalidParameterValueException"),
        "case \"a layer version nothing published\": expected the create refused, got {status} \
         {code:?}: {answered}"
    );
    assert_eq!(
        answered["message"].as_str(),
        Some(format!("Layer version {absent} does not exist.").as_str()),
        "case \"a layer version nothing published\": expected the absent-version sentence, got \
         {answered}"
    );
}
