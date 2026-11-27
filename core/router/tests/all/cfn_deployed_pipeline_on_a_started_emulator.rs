// SPDX-License-Identifier: CC0-1.0

//! The deployed S3-to-Lambda pipeline, driven against the emulator a customer STARTS: the
//! built binary, on its own port, with request authentication on, reached over TLS with
//! SigV4-signed calls.
//!
//! The rule under test: **a client that starts this emulator, deploys a stack declaring a
//! bucket whose `NotificationConfiguration` names a function, and then writes an object the
//! configuration admits, observes the handler's own effect — the whole chain holds on the
//! emulator as it is started, not only on a router a test assembles in its own process.**
//!
//! Every sibling suite of this pipeline drives a router built inside the test process, with
//! authentication off, on a runtime the test chose. The emulator a client runs is a different
//! configuration of the same code: the credentials are verified, each call arrives over its own
//! TLS connection, the deliveries are made on the binary's own multi-thread runtime, and one
//! process serves every call. This suite is the one that pins the chain THERE, so a pipeline
//! that works in-process and not on the started emulator is a failure a test reports rather
//! than one only a client sees.
//!
//! The emulator is started the way `tests/emulator_core` starts it — the built binary, a home
//! folder of its own, an idle timeout so a crashed test leaves nothing behind — and every
//! assertion quotes the emulator's own log, which is where a delivery that could not be made
//! says so. ONE deployment is driven here, since starting the binary costs what starting the
//! emulator costs; what the pipeline generalises over is spanned in process by the sibling
//! suite `cfn_deployed_s3_notification_to_lambda_pipeline`. For the same reason the one START
//! this suite pays for is SHARED: the leg in
//! [`crate::s3_notification_converts_on_a_started_emulator`] — the same chain wired by hand from
//! the function onwards, and filtered by a key-name `Filter` whose rules this suite's template
//! does not carry — runs on this emulator rather than starting a second one, as do the Auto
//! Scaling notification leg, the DynamoDB endpoint-discovery dial and the reason-scoped
//! EventBridge routing of a browser form upload.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::query_wire::{elements, form_encode, opt_element, outputs_of};

/// The Region the emulator's published configuration names, which is what an SDK signs for.
///
/// `pub(crate)` for the reason [`Started`] is: a leg running on the emulator this suite starts
/// addresses the same Region, and a second spelling of it could drift from this one.
pub(crate) const REGION: &str = "us-east-1";

/// The `StackStatus` the deployment must reach.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";

/// The prefix written into, and the prefix the handler writes its marker under.
const WATCHED_PREFIX: &str = "incoming/";
const PROCESSED_PREFIX: &str = "processed/";

/// How long a start may take to publish its files, how long the deployment and the delivery are
/// waited for, and how often this suite looks while waiting. Every wait is on a CONDITION; the
/// spans are generous, so a loaded host spends none of them.
const STARTUP_DEADLINE: Duration = Duration::from_secs(60);
const SETTLE_DEADLINE: Duration = Duration::from_secs(120);
const LOOK_INTERVAL: Duration = Duration::from_millis(200);

/// Ends an emulator a crashed test left behind, as `tests/emulator_core` does.
const ORPHAN_IDLE_TIMEOUT_SECS: &str = "120";

/// The handler the stack deploys: for the record it is handed it reads the object the event
/// named and writes one marker carrying what it read, through the AWS SDK the runtime supplies.
const PROCESSING_HANDLER: &str = r#"import json
import os
import urllib.parse

import boto3


def handler(event, context):
    s3 = boto3.client("s3")
    processed = []
    for record in event["Records"]:
        bucket = record["s3"]["bucket"]["name"]
        key = urllib.parse.unquote_plus(record["s3"]["object"]["key"])
        name = os.path.basename(key)
        body = s3.get_object(Bucket=bucket, Key=key)["Body"].read().decode("utf-8")
        s3.put_object(
            Bucket=os.environ["BUCKET_NAME"],
            Key="processed/" + name,
            Body=json.dumps({"key": key, "body": body}).encode("utf-8"),
        )
        processed.append(name)
    return {"processed": processed}
"#;

/// A running emulator, killed and its home folder removed once dropped, however a test ends.
///
/// `pub(crate)` because starting the built binary is the one harness no suite can write a second
/// time without drifting from this one: the sibling suite
/// [`crate::s3_notification_converts_on_a_started_emulator`] drives the same started emulator
/// through its own calls.
pub(crate) struct Started {
    child: Child,
    home: PathBuf,
    folder: PathBuf,
    port: u16,
    client: reqwest::blocking::Client,
    key: String,
    secret: String,
}

impl Drop for Started {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

impl Started {
    /// Start the built binary on a free port in a home folder of its own, and wait until it has
    /// published the files a client reads its port, its certificate and its credentials from.
    pub(crate) fn start() -> Self {
        let home = new_home();
        let log = std::fs::File::create(home.join("emulator.log")).expect("create the log file");
        let mut command = Command::new(emulator_binary());
        command.env_clear().env("HOME", &home);
        command.envs(std::env::var_os("PATH").map(|path| ("PATH", path)));
        command.env("EMULATOR_TELEMETRY", "off");
        command.args(["-p", "0", "--idle-timeout", ORPHAN_IDLE_TIMEOUT_SECS]);
        command.stdin(Stdio::null());
        command.stderr(log.try_clone().expect("duplicate the log file"));
        let child = command.stdout(log).spawn().expect("start the emulator binary");
        let mut started = Self {
            child,
            home,
            folder: PathBuf::new(),
            port: 0,
            client: reqwest::blocking::Client::new(),
            key: String::new(),
            secret: String::new(),
        };
        let giving_up_at = Instant::now() + STARTUP_DEADLINE;
        let folder = loop {
            if let Some(status) = started.child.try_wait().expect("read the emulator's state") {
                panic!(
                    "the emulator exited with {status}; its log:\n{}",
                    started.log()
                );
            }
            if let Some(folder) = folder_publishing_config(&started.home, 4) {
                break folder;
            }
            assert!(
                Instant::now() < giving_up_at,
                "the emulator published no files inside {STARTUP_DEADLINE:?}; its log:\n{}",
                started.log()
            );
            std::thread::sleep(LOOK_INTERVAL);
        };
        started.port = folder
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.parse().ok())
            .unwrap_or_else(|| panic!("{folder:?} names no port"));
        started.folder = folder;
        let credentials = std::fs::read_to_string(started.file("emulator_credentials"))
            .expect("read the published credentials");
        started.key = profile_value(&credentials, "aws_access_key_id")
            .expect("the published access key id");
        started.secret = profile_value(&credentials, "aws_secret_access_key")
            .expect("the published secret");
        started.client = trusting_client(&started.file("ca.pem"));
        started
    }

    fn file(&self, name: &str) -> PathBuf {
        self.folder.join(name)
    }

    /// Everything the emulator has written to its own log, which is where a delivery that could
    /// not be made, a throttled run or a refused call says so.
    pub(crate) fn log(&self) -> String {
        std::fs::read_to_string(self.home.join("emulator.log")).unwrap_or_default()
    }

    /// One signed call, answered with its status and body text.
    pub(crate) fn call(
        &self,
        signing_name: &str,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: Vec<u8>,
    ) -> (u16, String) {
        self.call_in_region(REGION, signing_name, method, path, headers, body)
    }

    /// The same call signed for — and addressed to the endpoint host of — `region`, which is
    /// what a client configured for another Region makes: the Region travels in the credential
    /// scope, and it is what names the Region the emulator serves the call in.
    pub(crate) fn call_in_region(
        &self,
        region: &str,
        signing_name: &str,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: Vec<u8>,
    ) -> (u16, String) {
        let dialled = format!("127.0.0.1:{}", self.port);
        let host = format!("{signing_name}.{region}.amazonaws.com");
        self.sent(
            (&dialled, &host, region),
            signing_name,
            method,
            path,
            headers,
            body,
        )
        .unwrap_or_else(|error| panic!("the emulator answers: {error}"))
    }

    /// One signed call DIALLED at `authority` rather than at the address this harness knows
    /// the emulator by: the `host:port` is both what the connection is made to and what the
    /// request carries as its `Host`, which is the position a client that was HANDED an
    /// address is in — it has that value and nothing else.
    ///
    /// `Err` carries the transport's own diagnosis when the dial itself failed, so an address
    /// nothing accepts a connection on is a case a suite can assert rather than a panic.
    pub(crate) fn call_at(
        &self,
        authority: &str,
        signing_name: &str,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: Vec<u8>,
    ) -> Result<(u16, String), String> {
        self.sent(
            (authority, authority, REGION),
            signing_name,
            method,
            path,
            headers,
            body,
        )
    }

    /// The one exchange both calls above are: the request is built for `path`, carries the
    /// `Host` header of `addressed` and the digest of its body, is signed for `signing_name`
    /// with the published key pair, and is sent over the connection the authority of
    /// `addressed` names — so neither caller can drift from the other on how a call is made.
    ///
    /// `addressed` is the triple (the authority the connection is made to, the `Host` the
    /// request carries, the Region the call is signed for): a client addressing an AWS endpoint
    /// host on a local emulator gives the first two different values, and the third travels in
    /// the credential scope.
    fn sent(
        &self,
        addressed: (&str, &str, &str),
        signing_name: &str,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: Vec<u8>,
    ) -> Result<(u16, String), String> {
        let (authority, host, region) = addressed;
        let uri = format!("https://{authority}{path}");
        let mut request = http::Request::builder().method(method).uri(&uri);
        request = request.header("host", host);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let digest = hex::encode(<sha2::Sha256 as sha2::Digest>::digest(&body));
        request = request.header("x-amz-content-sha256", digest);
        let request = request.body(body).expect("a well-formed request");
        let signed = self.signed(request, region, signing_name);
        let sendable = signed
            .try_into()
            .map_err(|error| format!("{uri} is not a request reqwest can send: {error}"))?;
        let response = self
            .client
            .execute(sendable)
            .map_err(|error| format!("nothing answered at {uri}: {error}"))?;
        let status = response.status().as_u16();
        Ok((status, response.text().unwrap_or_default()))
    }

    /// One UNSIGNED request, carrying `Host` and nothing of SigV4 — the position a browser
    /// submitting a form is in: it holds no key at all, and the credential it names travels in
    /// the form document rather than in an `Authorization` header (Amazon S3 API Reference,
    /// *POST Object*).
    ///
    /// `host` is what the request STATES it is addressed to, which a client that was handed an
    /// endpoint url and one that submits to a service host give different values; the connection
    /// is made to this emulator's own address either way ([`Self::authority`]).
    pub(crate) fn unsigned(
        &self,
        host: &str,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: Vec<u8>,
    ) -> (u16, String) {
        let uri = format!("https://{}{path}", self.authority());
        let mut request = http::Request::builder().method(method).uri(&uri);
        request = request.header("host", host);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let request = request.body(body).expect("a well-formed request");
        let response = self
            .client
            .execute(request.try_into().expect("a request reqwest can send"))
            .unwrap_or_else(|error| panic!("nothing answered at {uri}: {error}"));
        let status = response.status().as_u16();
        (status, response.text().unwrap_or_default())
    }

    /// The address this emulator answers on, which is the authority a client that was handed its
    /// published endpoint url carries as its `Host`.
    pub(crate) fn authority(&self) -> String {
        format!("127.0.0.1:{}", self.port)
    }

    /// The key pair this start published: what every call above is signed with, and what a form
    /// upload's own POST policy is signed with.
    pub(crate) fn credentials(&self) -> (&str, &str) {
        (&self.key, &self.secret)
    }

    /// The HTTPS client that trusts the certificate THIS start published — the certificate every
    /// listener of that process presents, its data port and each per-resource listener alike.
    ///
    /// `pub(crate)` for the reason [`Started`] is: a suite that has to DIAL an address this
    /// emulator PUBLISHED (a per-resource listener, which is not the address this harness knows
    /// the emulator by — [`Self::authority`]) with no credential at all, as a client handed a
    /// domain name does, needs this client rather than one of its own, which would trust nothing
    /// that start signed.
    pub(crate) fn client(&self) -> &reqwest::blocking::Client {
        &self.client
    }

    /// `request` signed with the published key pair for `signing_name`, as an SDK signs it.
    fn signed(
        &self,
        mut request: http::Request<Vec<u8>>,
        region: &str,
        signing_name: &str,
    ) -> http::Request<Vec<u8>> {
        use aws_sigv4::http_request::{sign, SignableBody, SignableRequest, SigningSettings};
        use aws_sigv4::sign::v4::SigningParams;

        let identity = aws_credential_types::Credentials::new(
            &self.key,
            &self.secret,
            None,
            None,
            "the published key pair",
        )
        .into();
        let params = SigningParams::builder()
            .identity(&identity)
            .region(region)
            .name(signing_name)
            .time(std::time::SystemTime::now())
            .settings(SigningSettings::default())
            .build()
            .expect("signing parameters")
            .into();
        let (method, uri) = (request.method().to_string(), request.uri().to_string());
        let headers: Vec<(String, String)> = request
            .headers()
            .iter()
            .map(|(name, value)| {
                (
                    name.as_str().to_string(),
                    value.to_str().unwrap_or_default().to_string(),
                )
            })
            .collect();
        let declared = headers
            .iter()
            .find(|(name, _)| name == "x-amz-content-sha256")
            .map(|(_, value)| value.clone());
        let borrowed = headers
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()));
        let bytes = SignableBody::Bytes(request.body());
        let body = declared.map_or(bytes, SignableBody::Precomputed);
        let signable =
            SignableRequest::new(&method, &uri, borrowed, body).expect("a signable request");
        let (instructions, _) = sign(signable, &params).expect("a signature").into_parts();
        instructions.apply_to_request_http1x(&mut request);
        request
    }

    /// One form-wire (`awsQuery` / `ec2Query`) call against the service whose signing name
    /// is `signing_name`, answered with its status and body text.
    ///
    /// The form-body assembly lives here, once, for the reason `query_wire` holds the
    /// encoder: a leg that writes its own drifts from the others.
    pub(crate) fn form_call(
        &self,
        signing_name: &str,
        version: &str,
        action: &str,
        params: &[(&str, &str)],
    ) -> (u16, String) {
        let mut body = format!("Action={action}&Version={version}");
        for (key, value) in params {
            body.push('&');
            body.push_str(&form_encode(key));
            body.push('=');
            body.push_str(&form_encode(value));
        }
        self.call(
            signing_name,
            "POST",
            "/",
            &[("content-type", "application/x-www-form-urlencoded")],
            body.into_bytes(),
        )
    }

    /// One `X-Amz-Target` RPC call against the service whose signing name is
    /// `signing_name`, answered with its status and body text.
    pub(crate) fn rpc(
        &self,
        signing_name: &str,
        target: &str,
        content_type: &str,
        body: &Value,
    ) -> (u16, String) {
        self.rpc_in_region(REGION, signing_name, target, content_type, body)
    }

    /// The same RPC call signed for `region` ([`Self::call_in_region`]).
    pub(crate) fn rpc_in_region(
        &self,
        region: &str,
        signing_name: &str,
        target: &str,
        content_type: &str,
        body: &Value,
    ) -> (u16, String) {
        self.call_in_region(
            region,
            signing_name,
            "POST",
            "/",
            &[("content-type", content_type), ("x-amz-target", target)],
            body.to_string().into_bytes(),
        )
    }

    /// One CloudFormation call over its awsQuery wire.
    pub(crate) fn cloudformation(&self, action: &str, members: &[(&str, &str)]) -> (u16, String) {
        self.form_call("cloudformation", "2010-05-15", action, members)
    }

    /// One call against S3's REST surface, path-style.
    pub(crate) fn s3(&self, method: &str, path: &str, body: &str) -> (u16, String) {
        self.call(
            "s3",
            method,
            path,
            &[("content-type", "application/octet-stream")],
            body.as_bytes().to_vec(),
        )
    }

    /// Everything the function logged, as `FilterLogEvents` reports it.
    pub(crate) fn logged(&self, function: &str) -> String {
        let group = format!("/aws/lambda/{function}");
        let (status, body) = self.call(
            "logs",
            "POST",
            "/",
            &[
                ("content-type", "application/x-amz-json-1.1"),
                ("x-amz-target", "Logs_20140328.FilterLogEvents"),
            ],
            json!({ "logGroupName": group }).to_string().into_bytes(),
        );
        format!("FilterLogEvents({group:?}) answered {status}: {body}")
    }
}

/// The binary Cargo built for this package, named at COMPILE time rather than looked up among
/// the variables of the running process: `CARGO_BIN_EXE_<name>` is the one thing this suite
/// needs of its environment, and the sibling suite that starts the same binary names it the
/// same way.
fn emulator_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_aws-emulator"))
}

/// A new folder to serve as an emulator's home: never one another user created first.
fn new_home() -> PathBuf {
    static STARTS: AtomicU32 = AtomicU32::new(0);
    let name = format!("emulator-pipeline-{}", std::process::id());
    let home = std::env::temp_dir().join(format!(
        "{name}-{}",
        STARTS.fetch_add(1, Ordering::Relaxed)
    ));
    let mut owner_only = std::fs::DirBuilder::new();
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut owner_only, 0o700);
    owner_only
        .create(&home)
        .map(|()| home)
        .expect("create the emulator's home folder")
}

/// The value of `name` in an AWS shared credentials file.
fn profile_value(text: &str, name: &str) -> Option<String> {
    let mut pairs = text.lines().filter_map(|line| line.split_once('='));
    let (_, value) = pairs.find(|(key, _)| key.trim() == name)?;
    Some(value.trim().to_string())
}

/// A blocking HTTPS client trusting only the certificate this start published.
fn trusting_client(ca_pem: &Path) -> reqwest::blocking::Client {
    use rustls::pki_types::pem::PemObject;

    let mut roots = rustls::RootCertStore::empty();
    let certificate =
        rustls::pki_types::CertificateDer::from_pem_file(ca_pem).expect("read the certificate");
    roots.add(certificate).expect("trust the certificate");
    let provider = std::sync::Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let builder = rustls::ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(rustls::ALL_VERSIONS)
        .expect("a TLS configuration");
    let tls = builder.with_root_certificates(roots).with_no_client_auth();
    reqwest::blocking::Client::builder()
        .use_preconfigured_tls(tls)
        .timeout(SETTLE_DEADLINE)
        .build()
        .expect("build the client")
}

/// The folder under `folder` holding a fully written `config`, published last, and not a link.
fn folder_publishing_config(folder: &Path, depth_left: usize) -> Option<PathBuf> {
    let config = folder.join("config");
    let is_file = std::fs::symlink_metadata(&config).is_ok_and(|file| file.is_file());
    if is_file && std::fs::read_to_string(config).is_ok_and(|text| text.ends_with('\n')) {
        return Some(folder.to_path_buf());
    }
    let entries = std::fs::read_dir(folder).ok().filter(|_| depth_left > 0)?;
    let is_folder = |entry: &std::fs::DirEntry| entry.file_type().is_ok_and(|kind| kind.is_dir());
    let mut folders = entries.flatten().filter(is_folder);
    folders.find_map(|entry| folder_publishing_config(&entry.path(), depth_left - 1))
}

/// The stack this suite deploys: the execution role, the function with its inline code, the
/// grant Amazon S3 needs on it, and the bucket whose notification configuration names the
/// function — ordered so the grant is in place before the bucket carries the configuration.
fn pipeline(bucket: &str) -> Value {
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": {
            "HandlerRole": {
                "Type": "AWS::IAM::Role",
                "Properties": {
                    "AssumeRolePolicyDocument": {
                        "Version": "2012-10-17",
                        "Statement": [{
                            "Effect": "Allow",
                            "Principal": { "Service": "lambda.amazonaws.com" },
                            "Action": "sts:AssumeRole",
                        }],
                    },
                    "Policies": [{
                        "PolicyName": "process-the-objects",
                        "PolicyDocument": {
                            "Version": "2012-10-17",
                            "Statement": [{
                                "Effect": "Allow",
                                "Action": ["s3:GetObject", "s3:PutObject", "logs:*"],
                                "Resource": "*",
                            }],
                        },
                    }],
                },
            },
            "Processor": {
                "Type": "AWS::Lambda::Function",
                "Properties": {
                    "Runtime": "python3.12",
                    "Handler": "index.handler",
                    "Role": { "Fn::GetAtt": ["HandlerRole", "Arn"] },
                    "Timeout": 30,
                    "Environment": { "Variables": { "BUCKET_NAME": bucket } },
                    "Code": { "ZipFile": PROCESSING_HANDLER },
                },
            },
            "InvokePermission": {
                "Type": "AWS::Lambda::Permission",
                "Properties": {
                    "FunctionName": { "Fn::GetAtt": ["Processor", "Arn"] },
                    "Action": "lambda:InvokeFunction",
                    "Principal": "s3.amazonaws.com",
                    "SourceArn": format!("arn:aws:s3:::{bucket}"),
                },
            },
            "DataBucket": {
                "Type": "AWS::S3::Bucket",
                "DependsOn": "InvokePermission",
                "Properties": {
                    "BucketName": bucket,
                    "NotificationConfiguration": {
                        "LambdaConfigurations": [{
                            "Event": "s3:ObjectCreated:*",
                            "Function": { "Fn::GetAtt": ["Processor", "Arn"] },
                            "Filter": {
                                "S3Key": {
                                    "Rules": [{ "Name": "prefix", "Value": WATCHED_PREFIX }],
                                },
                            },
                        }],
                    },
                },
            },
        },
        "Outputs": {
            "BucketName": { "Value": { "Ref": "DataBucket" } },
            "FunctionName": { "Value": { "Ref": "Processor" } },
        },
    })
}

/// Create `stack` from `template`, wait for a terminal status and answer its outputs. A stack
/// that did not complete is quoted with the emulator's own log.
pub(crate) fn deployed(
    emulator: &Started,
    case: &str,
    stack: &str,
    template: &str,
) -> std::collections::BTreeMap<String, String> {
    let (status, body) = emulator.cloudformation(
        "CreateStack",
        &[
            ("StackName", stack),
            ("TemplateBody", template),
            ("Capabilities.member.1", "CAPABILITY_IAM"),
        ],
    );
    assert_eq!(
        status, 200,
        "case {case:?}: CreateStack must be accepted, got {status} with {body}; the log:\n{}",
        emulator.log()
    );
    let giving_up_at = Instant::now() + SETTLE_DEADLINE;
    let mut described = String::new();
    let mut reported = String::new();
    while Instant::now() < giving_up_at {
        let (status, body) = emulator.cloudformation("DescribeStacks", &[("StackName", stack)]);
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeStacks must answer 200, got {status} with {body}"
        );
        reported = opt_element(&body, "StackStatus").unwrap_or_default();
        described = body;
        let settled = reported == CREATE_COMPLETE
            || reported.ends_with("_FAILED")
            || reported.contains("ROLLBACK");
        if settled {
            break;
        }
        std::thread::sleep(LOOK_INTERVAL);
    }
    assert_eq!(
        reported, CREATE_COMPLETE,
        "case {case:?}: the stack must reach {CREATE_COMPLETE}, got {reported:?} with \
         {described}; the log:\n{}",
        emulator.log()
    );
    outputs_of(&described)
}

/// The value `name` the stack published, or a failure naming what it did publish.
pub(crate) fn output(
    outputs: &std::collections::BTreeMap<String, String>,
    case: &str,
    name: &str,
) -> String {
    outputs.get(name).cloned().unwrap_or_else(|| {
        panic!("case {case:?}: the stack must output {name:?}, got {outputs:?}")
    })
}

/// The whole pipeline on the started emulator: the stack completes, the write the configuration
/// admits runs the function, and the marker the handler wrote reads back. Then, on the SAME
/// started emulator, the key-name filter leg of
/// [`crate::s3_notification_converts_on_a_started_emulator`], which needs a running emulator and
/// not a second one.
#[test]
fn a_deployed_pipeline_runs_on_the_emulator_a_client_starts() {
    let case = "the pipeline deployed on a started emulator";
    let emulator = Started::start();
    let stack = "started-emulator-pipeline";
    let bucket = format!("{stack}-data");

    let template = pipeline(&bucket).to_string();
    let outputs = deployed(&emulator, case, stack, &template);
    let deployed_bucket = output(&outputs, case, "BucketName");
    let function = output(&outputs, case, "FunctionName");

    let key = "incoming/sample.txt";
    let written = "the object the notification named";
    let (status, body) = emulator.s3("PUT", &format!("/{deployed_bucket}/{key}"), written);
    assert_eq!(
        status, 200,
        "case {case:?}: PutObject({key:?}) must succeed, got {status} with {body}"
    );

    let wanted = format!("{PROCESSED_PREFIX}sample.txt");
    let giving_up_at = Instant::now() + SETTLE_DEADLINE;
    let mut keys: Vec<String> = Vec::new();
    while Instant::now() < giving_up_at {
        let (status, body) = emulator.s3(
            "GET",
            &format!(
                "/{deployed_bucket}?list-type=2&prefix={}",
                form_encode(PROCESSED_PREFIX)
            ),
            "",
        );
        assert_eq!(
            status, 200,
            "case {case:?}: ListObjectsV2 must answer 200, got {status} with {body}"
        );
        keys = elements(&body, "Key");
        if keys.contains(&wanted) {
            break;
        }
        std::thread::sleep(LOOK_INTERVAL);
    }
    assert!(
        keys.contains(&wanted),
        "case {case:?}: the write of {key:?} must run the deployed function, which writes \
         {wanted:?} — got {keys:?}; {} ; the emulator's log:\n{}",
        emulator.logged(&function),
        emulator.log()
    );

    let (status, body) = emulator.s3("GET", &format!("/{deployed_bucket}/{wanted}"), "");
    assert_eq!(
        status, 200,
        "case {case:?}: GetObject({wanted:?}) must succeed, got {status} with {body}"
    );
    let marker: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    assert_eq!(
        marker["body"].as_str(),
        Some(written),
        "case {case:?}: the marker must carry the bytes the handler read back, got {body}"
    );

    // The second leg, on this same started emulator: a pipeline wired by hand from the function
    // onwards, whose notification configuration carries a key-name `Filter`.
    crate::s3_notification_converts_on_a_started_emulator::
        an_armed_notification_converts_only_what_its_literal_filter_admits(&emulator);

    // The third leg, on this same started emulator: an Auto Scaling group's own Amazon SNS
    // notification configurations, which publish through the same in-process callback this
    // binary makes every cross-service call over.
    crate::asg_notifications_on_a_started_emulator::
        an_asg_notification_reaches_its_subscribed_queue(&emulator);

    // The fourth leg, on this same started emulator: the authority DynamoDB endpoint
    // discovery announces, DIALLED — which only a listening process can answer, so it needs
    // a started emulator and not a second one.
    crate::dynamodb_discovered_endpoint_is_dialled_by_a_client::
        a_client_re_targeted_at_the_discovered_authority_is_served_there(&emulator);

    // The fifth leg, on this same started emulator: a MediaConvert job's own state changes,
    // announced on an EventBridge rule's Amazon SQS target by the binary's own clock with no
    // MediaConvert call waiting on them — which needs the started emulator, because the
    // publication is made from a task that outlives the request that armed the clock.
    crate::mediaconvert_job_state_change_on_a_started_emulator::
        a_jobs_state_changes_reach_its_rules_queue_on_the_emulator_a_client_starts(&emulator);

    // The sixth leg, on this same started emulator: the two creation paths of one
    // EventBridge-enabled bucket, routed by the reason their envelopes carry — which needs a
    // started emulator because the browser form upload it drives carries no `Authorization`
    // header at all, so which service claims it is decided on a request a listening process
    // received.
    crate::s3_form_post_reason_rules_on_a_started_emulator::
        reason_scoped_rules_split_the_creation_paths(&emulator);

    // The seventh leg, on this same started emulator: one bucket fanning its object events out
    // to a queue per event class, drained by long-polling consumers — which needs a started
    // emulator because each publication is signed with the credentials THAT process vends and
    // is made from a task that outlives the write that produced it.
    crate::s3_notification_routes_event_types_to_their_own_queues_on_a_started_emulator::
        per_event_class_entries_route_independently(&emulator);
}
