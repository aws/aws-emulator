// SPDX-License-Identifier: CC0-1.0

//! The emulator's HTTPS front door under a CONCURRENT, MULTI-CONNECTION client pool.
//!
//! RULE: the front door terminates TLS PER CONNECTION — each accepted socket owns its TLS
//! session and handshakes and is read independently of every other — so a client that drives
//! many SIMULTANEOUS connections out of one pool is answered, on every call, with an HTTP
//! response, and never with a transport-level failure. A transport answer is no answer at
//! all: it carries no status, no error code and no message, so no client retry policy and no
//! per-entry handling can run on it.
//!
//! On that response, `PutRecords` reports each entry of the batch SEPARATELY ("The response
//! Records array always includes the same number of records as the request array … A single
//! record failure does not stop the processing of subsequent records", `PutRecords`): an
//! entry the shard took carries its `SequenceNumber` and the `ShardId` it went to, an entry
//! the shard's write rate refused carries `ErrorCode`
//! `ProvisionedThroughputExceededException` with an `ErrorMessage` naming the stream and the
//! shard ("An ErrorCode value of ProvisionedThroughputExceededException has an error message
//! that includes the account ID, stream name, and shard ID",
//! `PutRecordsResultEntry$ErrorMessage`), and `FailedRecordCount` counts exactly the refused
//! entries. So a per-shard write-rate refusal is always visible INSIDE a 200, never as a
//! broken connection.
//!
//! The rows span what the rule generalises over: how many connections are in flight at once
//! (one, as a single-threaded control; eight; sixteen; thirty-two) and how large each call's
//! body is (a one-entry batch, a 500-entry batch of tiny records, a batch of four records
//! just under the per-record ceiling), and so whether the batch is inside what one shard
//! holds or past it. The recorded flow this was derived from is one row of that table —
//! sixteen threads out of one eighteen-connection pool, each putting 500 tiny entries back to
//! back. Every connection of a row starts its first call at the same instant (a barrier, not
//! a wait), so the row really does handshake several TLS sessions at once.
//!
//! This drives the emulator a customer runs: the BINARY this build produces, over its own
//! HTTPS data port, with requests signed by the keys that start publishes — which is where
//! the shared front door, its TLS terminator and its per-connection accept loop live. The
//! process harness and the signer below are this target's own: the equivalent harness under
//! `tests/emulator_core/` belongs to the core-shipped security suite, and a `#[path]` include
//! of it would pull that suite's other helpers into this target and couple this suite to a
//! file the core re-renders, so this one keeps the few lines it needs. That start gets a home
//! folder THIS suite creates, owner-only and named so no other run shares it, and it is
//! removed however the test ends; the client trusts only the certificate that start published
//! and no built-in root.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Barrier;
use std::time::{Duration, Instant, SystemTime};

use aws_credential_types::Credentials;
use aws_sigv4::http_request::{sign, SignableBody, SignableRequest, SigningSettings};
use aws_sigv4::sign::v4::SigningParams;
use serde_json::{json, Value};

/// The Region every call is signed for; the Region the start's published `config` names.
const REGION: &str = "us-east-1";

/// This service's SigV4 signing name, which is also the routing key the front door resolves
/// a signed request on.
const SIGNING_NAME: &str = "kinesis";

/// The `X-Amz-Target` prefix of this service's awsJson1_1 surface, and the content type that
/// protocol is framed in.
const TARGET_PREFIX: &str = "Kinesis_20131202";
const JSON_1_1: &str = "application/x-amz-json-1.1";

/// The `@length` maximum of `PutRecordsInput$Records`
/// (`smithy_query(query="operation",operation="PutRecords")` -> `Records` len 1..500).
const MAX_BATCH_RECORDS: usize = 500;

/// The `ErrorCode` an entry one shard could not take carries
/// (`PutRecordsResultEntry$ErrorCode`), spelled out here rather than imported so a wrong
/// constant in the implementation cannot make this suite pass.
const THROUGHPUT_EXCEEDED: &str = "ProvisionedThroughputExceededException";

/// The status a stream comes to rest in, and the mode this suite creates it in.
const ACTIVE: &str = "ACTIVE";
const PROVISIONED: &str = "PROVISIONED";

/// The one stream every row writes to: one shard, so every entry of every batch is metered
/// against the same shard and lands there.
const STREAM: &str = "https-front-door-concurrent-pooled-producer";

/// How long one call may take to come back. Generous: nothing here measures the wait, only
/// what came back, and a row deliberately keeps the emulator busy on many connections.
const ANSWER_DEADLINE: Duration = Duration::from_secs(120);

/// How long a start may take to publish its files, and how long a stream may take to reach
/// `ACTIVE`. Both generous for the same reason.
const STARTUP_DEADLINE: Duration = Duration::from_secs(120);
const ACTIVE_DEADLINE: Duration = Duration::from_secs(60);

/// One look inside a deadline. Every wait below polls for its CONDITION; none waits a fixed
/// time for something to have happened.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// Ends an emulator a crashed run left behind, without bounding anything this suite waits
/// for: it counts from the last API call, and every row makes one.
const ORPHAN_IDLE_TIMEOUT_SECS: &str = "60";

const LOG_FILE: &str = "emulator.log";

/// One mebibyte, the unit the published per-shard write-byte rate is expressed in.
const MI_B: usize = 1024 * 1024;

/// The `Data` of the largest record a row sends: a kibibyte under one mebibyte, because what
/// a record OCCUPIES is both of its members — "The record size limit applies to the total
/// size of the partition key and data blob" (`PutRecords`) — and the per-record ceiling of a
/// stream left at its default is one mebibyte. Four of these still exceed what one shard
/// holds at an instant.
const NEARLY_MI_B: usize = MI_B - 1024;

/// One row of the concurrency table: how many connections are in flight at once, how many
/// calls each of them makes, and the batch every call carries.
struct Row {
    case: &'static str,
    /// Connections in flight at once — one client pool, this many simultaneous TLS sessions.
    connections: usize,
    /// Calls each connection makes, back to back.
    calls: usize,
    /// Entries one call carries.
    entries: usize,
    /// Bytes of `Data` each entry carries.
    entry_bytes: usize,
    /// Whether the batch is past what ONE shard holds at one instant, so every call of the
    /// row MUST report refused entries inside its 200 — the half of the rule that says a
    /// write-rate refusal is a per-entry result and never a broken connection. True only
    /// where the capacity settles it whatever the host's speed.
    partially_refused: bool,
}

fn rows() -> [Row; 4] {
    [
        // The single-threaded control: one connection, the recorded batch size.
        Row {
            case: "one connection, 500 tiny entries",
            connections: 1,
            calls: 3,
            entries: MAX_BATCH_RECORDS,
            entry_bytes: 16,
            partially_refused: false,
        },
        // The recorded flow: many threads out of one pool, full batches of tiny records.
        Row {
            case: "sixteen simultaneous connections, 500 tiny entries",
            connections: 16,
            calls: 3,
            entries: MAX_BATCH_RECORDS,
            entry_bytes: 16,
            partially_refused: false,
        },
        // Twice as many connections, each carrying the smallest batch the model admits:
        // the handshakes, not the bodies, are what overlap.
        Row {
            case: "thirty-two simultaneous connections, one entry",
            connections: 32,
            calls: 2,
            entries: 1,
            entry_bytes: 16,
            partially_refused: false,
        },
        // Fewer connections, four near-mebibyte records on each: a body spanning many TLS
        // records while other connections are being read, and more than the two mebibytes
        // one shard holds at an instant (a second of the published write rate plus one
        // record of the stream's maximum), so every one of these calls is PARTIALLY refused
        // however fast the host runs — and still answered 200, per entry.
        Row {
            case: "eight simultaneous connections, four near-mebibyte records",
            connections: 8,
            calls: 2,
            entries: 4,
            entry_bytes: NEARLY_MI_B,
            partially_refused: true,
        },
    ]
}

/// A running emulator — the binary this build produced, started as a customer starts it, on
/// a free port and in a home folder of its own; killed and the folder removed on drop,
/// however the test ends.
struct Emulator {
    child: Child,
    home: PathBuf,
    /// The folder, named after the port, holding the files this start published.
    folder: PathBuf,
    port: u16,
}

impl Emulator {
    fn start() -> Self {
        let home = new_home();
        let log = std::fs::File::create(home.join(LOG_FILE)).expect("a log file in the home");
        let mut command = Command::new(env!("CARGO_BIN_EXE_aws-emulator"));
        // Only HOME and PATH come from this test's environment, so nothing set for the test
        // changes the run.
        command.env_clear().env("HOME", &home);
        command.envs(std::env::var_os("PATH").map(|path| ("PATH", path)));
        command.env("EMULATOR_TELEMETRY", "off");
        command.args(["-p", "0", "--idle-timeout", ORPHAN_IDLE_TIMEOUT_SECS]);
        command.stdin(Stdio::null()).stderr(log.try_clone().expect("the log clones"));
        let child = command.stdout(log).spawn().expect("the built emulator starts");
        let mut started = Self {
            child,
            home,
            folder: PathBuf::new(),
            port: 0,
        };
        let folder = wait_until(STARTUP_DEADLINE, || {
            if let Some(status) = started.child.try_wait().expect("the child is waitable") {
                panic!("the emulator exited with {status}:\n{}", started.log());
            }
            folder_publishing_config(&started.home, 4)
        });
        started.folder =
            folder.unwrap_or_else(|| panic!("the emulator published no files:\n{}", started.log()));
        let name = started.folder.file_name().and_then(|name| name.to_str());
        started.port = name
            .and_then(|name| name.parse().ok())
            .unwrap_or_else(|| panic!("{:?} names no port", started.folder));
        started
    }

    fn file(&self, name: &str) -> PathBuf {
        self.folder.join(name)
    }

    fn log(&self) -> String {
        std::fs::read_to_string(self.home.join(LOG_FILE)).unwrap_or_default()
    }

    /// The key id and secret in this start's published `emulator_credentials`: what a signed
    /// call must carry, since the emulator requires signatures unless started with `-a off`.
    fn published_key(&self) -> (String, String) {
        let path = self.file("emulator_credentials");
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("expected {path:?} published, got {error}"));
        let value = |name: &str| {
            let mut pairs = text.lines().filter_map(|line| line.split_once('='));
            let found = pairs.find(|(key, _)| key.trim() == name);
            found
                .map(|(_, value)| value.trim().to_string())
                .unwrap_or_else(|| panic!("expected {name} in {path:?}"))
        };
        (value("aws_access_key_id"), value("aws_secret_access_key"))
    }
}

impl Drop for Emulator {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

/// A new folder to serve as an emulator's home: owner-only, and one this suite CREATED — the
/// create fails rather than adopting an entry that is already there, so nothing another user
/// pre-placed is written into or followed.
fn new_home() -> PathBuf {
    let since_epoch = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH);
    let unique = since_epoch.map(|since| since.as_nanos()).unwrap_or_default();
    let name = format!("emulator-front-door-{}-{unique}", std::process::id());
    let home = std::env::temp_dir().join(name);
    let mut owner_only = std::fs::DirBuilder::new();
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut owner_only, 0o700);
    owner_only
        .create(&home)
        .map(|()| home)
        .expect("a home folder for the emulator that nothing else created")
}

/// The folder under `folder` holding a fully written `config`, published last, and not a
/// link — the folder named after the port this start bound.
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

/// Looks for `ready` until it answers, or `deadline` passes.
fn wait_until<T>(deadline: Duration, mut ready: impl FnMut() -> Option<T>) -> Option<T> {
    let give_up = Instant::now() + deadline;
    loop {
        match ready() {
            None if Instant::now() < give_up => std::thread::sleep(POLL_INTERVAL),
            outcome => return outcome,
        }
    }
}

/// The credential every call below signs with: the pair this start published.
struct Signer {
    key_id: String,
    secret: String,
}

impl Signer {
    /// Sign `request` for [`SIGNING_NAME`] in [`REGION`], in the `Authorization` header an
    /// SDK request carries.
    fn sign(&self, mut request: http::Request<Vec<u8>>) -> http::Request<Vec<u8>> {
        let identity = Credentials::new(&self.key_id, &self.secret, None, None, "test").into();
        let params = SigningParams::builder()
            .identity(&identity)
            .region(REGION)
            .name(SIGNING_NAME)
            .time(SystemTime::now())
            .settings(SigningSettings::default())
            .build()
            .expect("the signing parameters are complete")
            .into();
        let (method, uri) = (request.method().to_string(), request.uri().to_string());
        let headers: Vec<(String, String)> = request
            .headers()
            .iter()
            .map(|(name, value)| {
                (
                    name.as_str().to_string(),
                    value.to_str().expect("a text header").to_string(),
                )
            })
            .collect();
        let signable = SignableRequest::new(
            &method,
            &uri,
            headers
                .iter()
                .map(|(name, value)| (name.as_str(), value.as_str())),
            SignableBody::Bytes(request.body()),
        )
        .expect("the request is signable");
        let (instructions, _) = sign(signable, &params)
            .expect("the request signs")
            .into_parts();
        instructions.apply_to_request_http1x(&mut request);
        request
    }
}

/// A client pool trusting ONLY this start's `ca.pem`, holding up to `pool` connections to
/// the emulator — what a producer's connection pool is.
fn pooled_client(ca_pem: &Path, pool: usize) -> reqwest::blocking::Client {
    let pem = std::fs::read(ca_pem)
        .unwrap_or_else(|error| panic!("expected {ca_pem:?} published, got {error}"));
    let certificate = reqwest::Certificate::from_pem(&pem).expect("the published ca.pem parses");
    reqwest::blocking::Client::builder()
        .add_root_certificate(certificate)
        .tls_built_in_root_certs(false)
        .pool_max_idle_per_host(pool)
        .timeout(ANSWER_DEADLINE)
        .build()
        .expect("a client trusting this start")
}

/// One signed awsJson1_1 call on the emulator's HTTPS data port. `Err` is a TRANSPORT
/// failure — no status, no body — which is what this suite is about.
fn call(
    client: &reqwest::blocking::Client,
    port: u16,
    signer: &Signer,
    operation: &str,
    body: &Value,
) -> Result<(u16, Value), String> {
    let host = format!("{SIGNING_NAME}.{REGION}.amazonaws.com");
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://127.0.0.1:{port}/"))
        .header("host", &host)
        .header("content-type", JSON_1_1)
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .body(body.to_string().into_bytes())
        .expect("the test request is well formed");
    let request = signer.sign(request);
    let request: reqwest::blocking::Request =
        request.try_into().expect("the request converts to a client request");
    let response = client.execute(request).map_err(|error| error.to_string())?;
    let status = response.status().as_u16();
    let text = response.text().map_err(|error| error.to_string())?;
    let parsed = serde_json::from_str(&text).unwrap_or(Value::Null);
    Ok((status, parsed))
}

/// One call that must be answered `200`, for the setup this suite's rows run on.
fn ok(
    client: &reqwest::blocking::Client,
    port: u16,
    signer: &Signer,
    case: &str,
    operation: &str,
    body: &Value,
) -> Value {
    match call(client, port, signer, operation, body) {
        Ok((200, parsed)) => parsed,
        Ok((status, parsed)) => {
            panic!("case {case:?}: expected HTTP 200 from {operation}, got {status} with {parsed}")
        }
        Err(error) => panic!("case {case:?}: expected an answer from {operation}, got {error}"),
    }
}

/// What a `PutRecords` answer must be: a 200 whose `Records` holds one result per entry,
/// each either stored (a sequence number and the shard it went to) or refused by the shard's
/// write rate (the modelled code and a message naming the account, the stream and the
/// shard), with `FailedRecordCount` counting exactly the refused ones. Answers how many
/// entries were refused, so a row past the shard's capacity can be held to reporting them.
fn assert_put_records_answer(
    case: &str,
    connection: usize,
    index: usize,
    entries: usize,
    stream: &str,
    shard: &str,
    outcome: Result<(u16, Value), String>,
) -> usize {
    let where_ = format!("case {case:?}, connection {connection}, call {index}");
    let (status, answer) = outcome.unwrap_or_else(|error| {
        panic!(
            "{where_}: expected an HTTP answer to every call of a pooled client, got the \
             transport failure {error}"
        )
    });
    assert_eq!(
        status, 200,
        "{where_}: expected PutRecords answered 200 with one result per entry, got {status} \
         with {answer}"
    );
    let results = answer["Records"].as_array().unwrap_or_else(|| {
        panic!("{where_}: expected a Records list of per-entry results, got {answer}")
    });
    assert_eq!(
        results.len(),
        entries,
        "{where_}: expected one result per submitted entry ({entries}), got {}",
        results.len()
    );
    let mut refused = 0usize;
    for (position, result) in results.iter().enumerate() {
        match result["ErrorCode"].as_str() {
            None => {
                assert!(
                    result["SequenceNumber"].as_str().is_some_and(|at| !at.is_empty()),
                    "{where_}: a stored entry names its sequence number, entry {position} is \
                     {result}"
                );
                assert_eq!(
                    result["ShardId"].as_str(),
                    Some(shard),
                    "{where_}: a stored entry names the shard it went to, entry {position} is \
                     {result}"
                );
            }
            Some(code) => {
                refused += 1;
                assert_eq!(
                    code, THROUGHPUT_EXCEEDED,
                    "{where_}: an entry the shard's write rate refused carries the modelled \
                     code, entry {position} is {result}"
                );
                let message = result["ErrorMessage"].as_str().unwrap_or_default();
                let names_account = message.contains(::engine::EMULATOR_ACCOUNT_ID);
                assert!(
                    names_account && message.contains(stream) && message.contains(shard),
                    "{where_}: expected a per-entry message naming the account, the stream \
                     and the shard, entry {position} is {result}"
                );
                assert!(
                    result["SequenceNumber"].is_null() && result["ShardId"].is_null(),
                    "{where_}: an entry that was not stored names no position, entry \
                     {position} is {result}"
                );
            }
        }
    }
    let reported = answer["FailedRecordCount"].as_i64().unwrap_or(0);
    assert_eq!(
        reported,
        i64::try_from(refused).unwrap_or(-1),
        "{where_}: expected FailedRecordCount to count the entries that were not stored \
         ({refused}), got {reported}"
    );
    refused
}

/// The batch one call of `row` carries: every entry under one partition key, so every entry
/// maps to the one shard of the stream.
fn batch(row: &Row) -> Value {
    let data = aws_smithy_types::base64::encode(vec![b'x'; row.entry_bytes]);
    let records: Vec<Value> = (0..row.entries)
        .map(|_| json!({"PartitionKey": "one-shard", "Data": &data}))
        .collect();
    json!({"StreamName": STREAM, "Records": records})
}

/// The one shard of a one-shard stream, once the stream is `ACTIVE`.
fn the_only_shard(
    client: &reqwest::blocking::Client,
    port: u16,
    signer: &Signer,
    case: &str,
) -> String {
    let listed = ok(
        client,
        port,
        signer,
        case,
        "ListShards",
        &json!({"StreamName": STREAM}),
    );
    let shards = listed["Shards"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected ListShards to answer a list, got {listed}"));
    assert_eq!(
        shards.len(),
        1,
        "case {case:?}: expected the stream to have exactly one shard, got {listed}"
    );
    shards[0]["ShardId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a shard id, got {listed}"))
        .to_string()
}

#[test]
fn every_call_of_a_concurrent_pooled_producer_is_answered_over_the_https_front_door() {
    let emulator = Emulator::start();
    let port = emulator.port;
    let (key_id, secret) = emulator.published_key();
    let signer = &Signer { key_id, secret };
    // One pool, wide enough for the busiest row, shared by every connection of every row —
    // the client a producer builds once.
    let widest = rows().iter().map(|row| row.connections).max().unwrap_or(1);
    let client = &pooled_client(&emulator.file("ca.pem"), widest);

    let setup = "the one-shard stream every row writes to";
    ok(
        client,
        port,
        signer,
        setup,
        "CreateStream",
        &json!({
            "StreamName": STREAM,
            "ShardCount": 1,
            "StreamModeDetails": {"StreamMode": PROVISIONED},
        }),
    );
    // Wait for the CONDITION the stream reaching ACTIVE is, never for a fixed time.
    let active = wait_until(ACTIVE_DEADLINE, || {
        let summary = ok(
            client,
            port,
            signer,
            setup,
            "DescribeStreamSummary",
            &json!({"StreamName": STREAM}),
        );
        let status = summary["StreamDescriptionSummary"]["StreamStatus"].as_str();
        (status == Some(ACTIVE)).then(|| status.unwrap_or_default().to_string())
    });
    assert_eq!(
        active.as_deref(),
        Some(ACTIVE),
        "case {setup:?}: expected the stream to reach {ACTIVE} before any producer writes"
    );
    let shard = &the_only_shard(client, port, signer, setup);

    for row in rows() {
        let body = &batch(&row);
        // Every connection of the row opens and handshakes at the same instant: what the
        // rule is about is several TLS sessions in flight at once, not the calls' content.
        let start_together = &Barrier::new(row.connections);
        std::thread::scope(|scope| {
            let producers: Vec<_> = (0..row.connections)
                .map(|connection| {
                    let row = &row;
                    scope.spawn(move || {
                        start_together.wait();
                        for index in 0..row.calls {
                            let outcome = call(client, port, signer, "PutRecords", body);
                            let refused = assert_put_records_answer(
                                row.case,
                                connection,
                                index,
                                row.entries,
                                STREAM,
                                shard,
                                outcome,
                            );
                            assert!(
                                refused > 0 || !row.partially_refused,
                                "case {:?}, connection {connection}, call {index}: a batch \
                                 past what one shard holds at an instant reports the entries \
                                 it could not take inside its 200, got none refused of {}",
                                row.case,
                                row.entries
                            );
                        }
                    })
                })
                .collect();
            for producer in producers {
                producer
                    .join()
                    .unwrap_or_else(|_| panic!("case {:?}: a producer ended early", row.case));
            }
        });
    }

    // The front door survived every row: it still handshakes a BRAND NEW pool and answers
    // both the readiness probe and another batch on it.
    let after = "after the concurrent rows";
    let fresh = &pooled_client(&emulator.file("ca.pem"), 1);
    let health = fresh
        .get(format!("https://127.0.0.1:{port}/__admin/health"))
        .send()
        .unwrap_or_else(|error| panic!("case {after:?}: expected health answered, got {error}"));
    let (status, text) = (health.status().as_u16(), health.text().unwrap_or_default());
    assert_eq!(
        (status, text.as_str()),
        (200, "ok"),
        "case {after:?}: expected the data port's readiness probe to answer 200 ok, got \
         {status} with {text:?}"
    );
    let row = Row {
        case: after,
        connections: 1,
        calls: 1,
        entries: 1,
        entry_bytes: 16,
        partially_refused: false,
    };
    let outcome = call(fresh, port, signer, "PutRecords", &batch(&row));
    assert_put_records_answer(after, 0, 0, 1, STREAM, shard, outcome);
}
