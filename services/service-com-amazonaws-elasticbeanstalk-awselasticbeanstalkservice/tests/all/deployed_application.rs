// SPDX-License-Identifier: CC0-1.0

//! The APPLICATION an environment runs, over the service's own HTTP surface.
//!
//! The rule under test: an environment RUNS the application version deployed to
//! it, so the deployed code's own output is what the service reports — the log
//! lines it writes are the lines the compiled environment information carries
//! beside this service's activity log, and the response its own request handler
//! ends is what the address the environment publishes (`CNAME`/`EndpointURL`,
//! which is an address this emulator answers on) serves. Nothing about either is
//! synthesized: a bundle that starts no server of its own serves the FILES it
//! carries, and a bundle with neither serves nothing at all.
//!
//! The table spans what that rule generalises over: how the bundle names its
//! entry point (`app.js`, a `package.json` `main`, a Python platform's
//! `WSGIPath`), the LANGUAGE that entry point is written in and so the engine it
//! runs in, what the code does (logs, answers a request, reads an environment
//! property it was configured with, answers a path-dependent body), a bundle with
//! no runnable entry point at all (static files only), and one with neither an
//! entry point nor a document for the path.
//!
//! Where that output GOES is part of the same rule: an environment whose
//! `aws:elasticbeanstalk:cloudwatch:logs` configuration turns instance log
//! streaming on publishes a CloudWatch Logs group per on-instance log file its
//! platform streams, keeps each for the retention that configuration states, and
//! delivers the application's own standard output into the group of the file it
//! goes to — which the last suites here assert where a client reads them, in
//! CloudWatch Logs itself.
//!
//! The HTTP harness (the router, the form encoder, the XML element reader, the
//! object-storage upload and the unsigned GET) is the one `lifecycle.rs` already
//! owns in this same test binary.

use crate::application_version_processing::Bundle;
use crate::environment_info::entries;
use crate::lifecycle::{
    assert_published_cname, wait_for_status, Answer, Emulator, ACCOUNT_KEY_ID, PAST_DUE,
    POLL_INTERVAL, SOLUTION_STACK, STATUS_DEADLINE, TEST_REGION,
};

use service_com_amazonaws_elasticbeanstalk_awselasticbeanstalkservice as eb;

/// The application every environment below is launched under.
const APPLICATION: &str = "deployed-app";

/// The bucket the suites upload a source bundle into, and the key under it.
const BUNDLE_BUCKET: &str = "eb-deployed-bundles";
const BUNDLE_KEY: &str = "bundles/app.zip";

/// The environment property the launches below configure, and its value: a
/// setting of `aws:elasticbeanstalk:application:environment` is an environment
/// property the application reads out of its own process environment.
const PROPERTY_NAME: &str = "GREETING";
const PROPERTY_VALUE: &str = "hello from the environment";

/// A marker the deployed code logs, carrying non-ASCII text so an encoding
/// regression anywhere in the log path is visible: the bytes the application
/// wrote must come back byte for byte.
const LOG_MARKER: &str = "started: naïve ✓ Ünicode marker ✔";

/// A Node.js application that logs [`LOG_MARKER`] while starting and answers
/// every request with the environment property it was configured with and the
/// path it was asked for.
const APP_JS: &[u8] = br#"
const http = require('http');
console.log('started: nai\u0308ve \u2713 U\u0308nicode marker \u2714'.normalize('NFC'));
http.createServer(function (req, res) {
  res.writeHead(200, { 'Content-Type': 'text/plain; charset=utf-8' });
  res.end(process.env.GREETING + ' at ' + req.url);
}).listen(process.env.PORT, function () {
  console.log('listening on ' + process.env.PORT);
});
"#;

/// The same application under a name only the bundle's `package.json` names.
const MAIN_JS: &[u8] = br#"
const http = require('http');
console.log('started: nai\u0308ve \u2713 U\u0308nicode marker \u2714'.normalize('NFC'));
http.createServer((req, res) => {
  res.statusCode = 200;
  res.setHeader('content-type', 'text/plain; charset=utf-8');
  res.write(process.env.GREETING);
  res.end(' at ' + req.url);
}).listen(Number(process.env.PORT));
"#;

/// A `package.json` whose `main` names that file.
const PACKAGE_JSON: &[u8] = br#"{"name":"demo","version":"1.0.0","main":"./src/main.js"}"#;

/// A document a bundle with no entry point serves its root from.
const INDEX_HTML: &[u8] = b"<!doctype html><title>deployed</title><p>static content\n";

/// Code that never finishes starting: an endless loop before anything is served.
const ENDLESS_LOOP_JS: &[u8] = b"while (true) { }\n";

/// Code with no base case, which the engine's stack bound is what stops.
const ENDLESS_RECURSION_JS: &[u8] = b"function down(n) { return down(n + 1); }\ndown(0);\n";

/// Code that writes without end, which the cap on captured output is what stops.
const ENDLESS_OUTPUT_JS: &[u8] =
    b"for (var i = 0; ; i++) { console.log('flood ' + i + ' ----------------'); }\n";

/// One bundle per way deployed code can fail to finish.
const ENDLESS_LOOP_BUNDLE: &[(&str, &[u8])] = &[("app.js", ENDLESS_LOOP_JS)];
/// See [`ENDLESS_LOOP_BUNDLE`].
const ENDLESS_RECURSION_BUNDLE: &[(&str, &[u8])] = &[("app.js", ENDLESS_RECURSION_JS)];
/// See [`ENDLESS_LOOP_BUNDLE`].
const ENDLESS_OUTPUT_BUNDLE: &[(&str, &[u8])] = &[("app.js", ENDLESS_OUTPUT_JS)];

/// How long a request to an environment running code that never finishes may
/// take before this suite calls it unbounded. Well above the engine's own
/// wall-clock bound, so a loaded machine does not fail the suite, and well below
/// a hang.
const BOUNDED_RUN: std::time::Duration = std::time::Duration::from_secs(30);

/// How much of an answer this suite accepts as bounded: the engine caps what one
/// run captures, so neither the compiled log nor the response can grow with the
/// code that wrote it.
const BOUNDED_BYTES: usize = 1024 * 1024;

/// One deployed bundle, and what the environment running it must then report.
struct Row {
    case: &'static str,
    bundle: Bundle,
    /// The status a GET of the environment's own address must answer.
    status: u16,
    /// What that answer's body must carry.
    body: Option<&'static str>,
    /// Whether the compiled environment information must carry the
    /// application's own log line.
    logs: bool,
}

/// One row per thing the rule generalises over.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "an app.js that logs and serves",
            bundle: Bundle::Zip(&[("app.js", APP_JS)]),
            status: 200,
            body: Some(PROPERTY_VALUE),
            logs: true,
        },
        Row {
            case: "an entry point named by package.json main",
            bundle: Bundle::Zip(&[
                ("package.json", PACKAGE_JSON),
                ("src/main.js", MAIN_JS),
                ("index.html", INDEX_HTML),
            ]),
            status: 200,
            body: Some(PROPERTY_VALUE),
            logs: true,
        },
        Row {
            case: "a bundle of static files only",
            bundle: Bundle::Zip(&[("index.html", INDEX_HTML)]),
            status: 200,
            body: Some("static content"),
            logs: false,
        },
        Row {
            case: "a bundle with neither an entry point nor a root document",
            bundle: Bundle::Zip(&[("README.txt", b"nothing runnable here\n")]),
            status: 404,
            body: None,
            logs: false,
        },
    ]
}

/// The NAME a client dials to reach the environment carrying the CNAME prefix
/// `prefix`: a name under the zone this emulator announces its addresses in, which
/// the plane serves for whichever environment carries that prefix now. The
/// environment publishes the same prefix under a sub-domain of its own, which only
/// the service can mint (`published_environment_name.rs` is where that value's
/// form is asserted).
fn published_address(emulator: &Emulator, prefix: &str) -> String {
    emulator.dialable_name(TEST_REGION, prefix)
}

/// The URL this suite DIALS for `prefix` and `path` — the published name, with the
/// request path under it, which is the address a client dials as it was given it.
fn dialled(emulator: &Emulator, prefix: &str, path: &str) -> String {
    format!("http://{}{path}", published_address(emulator, prefix))
}

/// Call `action`, failing the named case unless it answered 200.
async fn ok(emulator: &Emulator, case: &str, action: &str, params: &[(&str, &str)]) -> Answer {
    let answer = emulator.call(action, params).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected 200 from {action}, got {} and body {}",
        answer.status, answer.body
    );
    answer
}

/// Upload `bundle`, create the application and a version from it, launch an
/// environment deploying that version with the environment property configured,
/// and wait out the launch the clock owes.
async fn deployed(emulator: &Emulator, case: &str, prefix: &str, bundle: Bundle) {
    emulator
        .put_object(BUNDLE_BUCKET, BUNDLE_KEY, &bundle.bytes())
        .await;
    ok(
        emulator,
        case,
        "CreateApplication",
        &[("ApplicationName", APPLICATION)],
    )
    .await;
    ok(
        emulator,
        case,
        "CreateApplicationVersion",
        &[
            ("ApplicationName", APPLICATION),
            ("VersionLabel", "v1"),
            ("SourceBundle.S3Bucket", BUNDLE_BUCKET),
            ("SourceBundle.S3Key", BUNDLE_KEY),
        ],
    )
    .await;
    let launched = ok(
        emulator,
        case,
        "CreateEnvironment",
        &[
            ("ApplicationName", APPLICATION),
            ("EnvironmentName", "deployed-env-1"),
            ("CNAMEPrefix", prefix),
            ("SolutionStackName", SOLUTION_STACK.as_str()),
            ("VersionLabel", "v1"),
            (
                "OptionSettings.member.1.Namespace",
                eb::state::NAMESPACE_APPLICATION_ENVIRONMENT,
            ),
            ("OptionSettings.member.1.OptionName", PROPERTY_NAME),
            ("OptionSettings.member.1.Value", PROPERTY_VALUE),
        ],
    )
    .await;
    let address = published_address(emulator, prefix);
    // The launch has not been assigned its address yet, so it reports none; the
    // name the environment publishes is read once the launch has finished, below.
    assert!(
        launched.value("CNAME").is_none(),
        "case {case:?}: a launch still provisioning reports no CNAME, got {}",
        launched.body
    );
    // The `EndpointURL` beside the name is the address of what answers BEHIND it,
    // which is a host too — never the name, and never a host carrying a port or a
    // path. WHICH address it is (the environment's load balancer, or its single
    // instance) is `published_environment_name.rs`'s table.
    let endpoint = launched.expect("EndpointURL", case);
    assert!(
        !endpoint.contains(':') && !endpoint.contains('/') && endpoint != address,
        "case {case:?}: the EndpointURL must be the host of what answers behind the name, got \
         {endpoint:?} and body {}",
        launched.body
    );
    // The launch the clock owes is waited for as the CONDITION the environment
    // reaches, never as a fixed time: the application behind the address is
    // deployed by it, and the name it publishes is assigned when it finishes.
    wait_for_status(emulator, case, "deployed-env-1", eb::state::STATUS_READY).await;
    // The environment publishes the name its prefix forms, under a sub-domain of
    // its own: the prefix is the first label either way, which is what the address
    // above dials.
    let described = ok(
        emulator,
        case,
        "DescribeEnvironments",
        &[("EnvironmentNames.member.1", "deployed-env-1")],
    )
    .await;
    assert_published_cname(
        case,
        described.expect("Environments/member/CNAME", case),
        TEST_REGION,
        prefix,
    );
}

/// RULE: the address an environment publishes is served by the application the
/// deployed version carries — the response its own handler ends, or the file of
/// its bundle the request addresses — and serving it changes nothing a later
/// caller sees.
#[tokio::test]
async fn the_published_address_is_served_by_the_deployed_application() {
    for row in rows() {
        let case = row.case;
        let emulator = Emulator::with_object_storage();
        deployed(&emulator, case, "deployed-env-1", row.bundle).await;

        let url = dialled(&emulator, "deployed-env-1", "/");
        let served = emulator.get_url(&url).await;
        assert_eq!(
            served.status, row.status,
            "case {case:?}: GET {url} must answer {}, got {} and body {}",
            row.status, served.status, served.body
        );
        if let Some(expected) = row.body {
            assert!(
                served.body.contains(expected),
                "case {case:?}: the answer must carry what the deployed bundle produces \
                 ({expected:?}), got {:?}",
                served.body
            );
        }
        // A read changes nothing a later caller sees: the same request is
        // answered the same way.
        let again = emulator.get_url(&url).await;
        assert_eq!(
            again.body, served.body,
            "case {case:?}: two reads of the address must answer alike, got {:?} then {:?}",
            served.body, again.body
        );
        assert_eq!(
            again.status, served.status,
            "case {case:?}: two reads of the address must answer the same status, got {} then {}",
            served.status, again.status
        );
    }
}

/// RULE: the compiled environment information carries the deployed
/// application's own log lines, byte for byte, beside this service's activity
/// log — and carries only the activity log for an environment running no
/// application of its own.
#[tokio::test]
async fn the_compiled_information_carries_the_applications_own_log() {
    for row in rows() {
        let case = row.case;
        let emulator = Emulator::with_object_storage();
        deployed(&emulator, case, "deployed-env-1", row.bundle).await;

        ok(
            &emulator,
            case,
            "RequestEnvironmentInfo",
            &[
                ("EnvironmentName", "deployed-env-1"),
                ("InfoType", eb::state::ENVIRONMENT_INFO_TYPE_TAIL),
            ],
        )
        .await;
        std::thread::sleep(PAST_DUE);
        let retrieved = ok(
            &emulator,
            case,
            "RetrieveEnvironmentInfo",
            &[
                ("EnvironmentName", "deployed-env-1"),
                ("InfoType", eb::state::ENVIRONMENT_INFO_TYPE_TAIL),
            ],
        )
        .await;
        let compiled = entries(&retrieved);
        let entry = compiled
            .first()
            .unwrap_or_else(|| panic!("case {case:?}: expected an entry, got {compiled:?}"));
        let downloaded = emulator.get_url(&entry.message).await;
        assert_eq!(
            downloaded.status, 200,
            "case {case:?}: the compiled file must download, got {} and body {}",
            downloaded.status, downloaded.body
        );
        // This service's own activity is in the compiled information whatever
        // the environment runs.
        assert!(
            downloaded.body.contains("createEnvironment"),
            "case {case:?}: the compiled file must carry the activity log, got {:?}",
            downloaded.body
        );
        assert_eq!(
            downloaded.body.contains(LOG_MARKER),
            row.logs,
            "case {case:?}: the application's own log line ({LOG_MARKER:?}) must be carried \
             exactly when the bundle runs one, got {:?}",
            downloaded.body
        );
        assert_eq!(
            downloaded.body.contains(eb::state::APPLICATION_STDOUT_LOG),
            row.logs,
            "case {case:?}: the application's log file is named exactly when there is one, \
             got {:?}",
            downloaded.body
        );
    }
}

/// RULE: only a LIVE environment's address is served. An address no environment
/// carries is served by nothing, and an environment the clock has terminated
/// ("Terminated: Environment is not running") stops serving the address it held.
#[tokio::test]
async fn an_address_is_served_only_while_an_environment_carries_it() {
    let case = "terminated";
    let emulator = Emulator::with_object_storage();
    deployed(
        &emulator,
        case,
        "deployed-env-1",
        Bundle::Zip(&[("app.js", APP_JS)]),
    )
    .await;
    let url = dialled(&emulator, "deployed-env-1", "/");

    // An address no environment ever took.
    let unknown = dialled(&emulator, "never-taken", "/");
    let missing = emulator.get_url(&unknown).await;
    assert_eq!(
        missing.status, 404,
        "case {case:?}: an address nothing carries must not be served, got {} and body {}",
        missing.status, missing.body
    );

    let live = emulator.get_url(&url).await;
    assert_eq!(
        live.status, 200,
        "case {case:?}: the live environment's address must be served, got {} and body {}",
        live.status, live.body
    );

    ok(
        &emulator,
        case,
        "TerminateEnvironment",
        &[("EnvironmentName", "deployed-env-1")],
    )
    .await;
    std::thread::sleep(PAST_DUE);
    let terminated = emulator.get_url(&url).await;
    assert_eq!(
        terminated.status, 404,
        "case {case:?}: a terminated environment must stop serving its address, got {} and \
         body {}",
        terminated.status, terminated.body
    );
}

/// RULE: a request's own path and method reach the application: the response is
/// computed by the deployed code from the request it was handed, so a different
/// path is answered differently.
#[tokio::test]
async fn the_application_answers_the_request_it_was_handed() {
    let case = "request path";
    let emulator = Emulator::with_object_storage();
    deployed(
        &emulator,
        case,
        "deployed-env-1",
        Bundle::Zip(&[("app.js", APP_JS)]),
    )
    .await;

    // One row per path the client asks for, including a query string, which
    // Node.js carries on `req.url`.
    for path in ["/", "/status", "/items?page=2"] {
        let url = dialled(&emulator, "deployed-env-1", path);
        let served = emulator.get_url(&url).await;
        assert_eq!(
            served.status, 200,
            "case {case:?} {path:?}: expected 200, got {} and body {}",
            served.status, served.body
        );
        assert!(
            served.body.contains(&format!("at {path}")),
            "case {case:?} {path:?}: the application must answer from the request it was \
             handed, got {:?}",
            served.body
        );
    }
}

/// RULE: the engine the deployed code runs on is BOUNDED before anything is
/// evaluated — a step budget, a wall-clock deadline, a stack bound and a cap on
/// what one run captures — so code that never finishes is STOPPED and reported as
/// an application that answered nothing, rather than hanging the request it was
/// handed or growing what it wrote without end.
///
/// Every assertion here is reached only because the run was stopped: a run that
/// was not would never answer at all.
#[tokio::test]
async fn code_that_never_finishes_is_stopped_and_reported() {
    // One row per way deployed code can fail to finish: an endless loop (the
    // step budget and the deadline), recursion with no base case (the stack
    // bound), and output with no end (the cap on what is captured).
    let rows: &[(&str, Bundle)] = &[
        ("an endless loop", Bundle::Zip(ENDLESS_LOOP_BUNDLE)),
        (
            "recursion with no base case",
            Bundle::Zip(ENDLESS_RECURSION_BUNDLE),
        ),
        ("output with no end", Bundle::Zip(ENDLESS_OUTPUT_BUNDLE)),
    ];
    for (case, bundle) in rows {
        let emulator = Emulator::with_object_storage();
        deployed(&emulator, case, "deployed-env-1", *bundle).await;

        // The address answers: the run was stopped, and nothing it never
        // produced is reported as a response.
        let started = std::time::Instant::now();
        let served = emulator
            .get_url(&dialled(&emulator, "deployed-env-1", "/"))
            .await;
        let elapsed = started.elapsed();
        assert!(
            elapsed < BOUNDED_RUN,
            "case {case:?}: the request must be answered inside the engine's own bounds, \
             took {elapsed:?}"
        );
        assert_ne!(
            served.status, 200,
            "case {case:?}: code that never answered a request must not produce a success, \
             got body {:?}",
            served.body
        );
        assert!(
            served.body.is_empty(),
            "case {case:?}: no body is invented for an application that answered nothing, \
             got {:?}",
            served.body
        );

        // And the information compiled from such an environment is bounded too:
        // what the run wrote before it was stopped, and no more.
        ok(
            &emulator,
            case,
            "RequestEnvironmentInfo",
            &[
                ("EnvironmentName", "deployed-env-1"),
                ("InfoType", eb::state::ENVIRONMENT_INFO_TYPE_TAIL),
            ],
        )
        .await;
        std::thread::sleep(PAST_DUE);
        let retrieved = ok(
            &emulator,
            case,
            "RetrieveEnvironmentInfo",
            &[
                ("EnvironmentName", "deployed-env-1"),
                ("InfoType", eb::state::ENVIRONMENT_INFO_TYPE_TAIL),
            ],
        )
        .await;
        let compiled = entries(&retrieved);
        let entry = compiled
            .first()
            .unwrap_or_else(|| panic!("case {case:?}: expected an entry, got {compiled:?}"));
        let downloaded = emulator.get_url(&entry.message).await;
        assert_eq!(
            downloaded.status, 200,
            "case {case:?}: the compiled file must download, got {} and body {}",
            downloaded.status, downloaded.body
        );
        assert!(
            downloaded.body.len() < BOUNDED_BYTES,
            "case {case:?}: the compiled log must be bounded by what one run may capture, \
             got {} bytes",
            downloaded.body.len()
        );
        assert!(
            downloaded.body.contains("createEnvironment"),
            "case {case:?}: the compiled file must still carry the activity log, got {} bytes",
            downloaded.body.len()
        );
    }
}

/// The Python module and the WSGI callable in it that one row deploys, and the
/// `WSGIPath` the environment is configured with — the three things that decide
/// which file the platform starts the application from.
struct PythonRow {
    case: &'static str,
    /// The path the module is stored at inside the bundle.
    module_file: &'static str,
    /// The name the WSGI callable is bound to in it.
    callable: &'static str,
    /// The `aws:elasticbeanstalk:container:python` `WSGIPath` the environment
    /// sets, or `None` for a configuration that sets none and leaves the platform
    /// starting the application from the value the option catalog publishes.
    wsgi_path: Option<&'static str>,
}

/// One row per way a Python bundle's entry point is named.
fn python_rows() -> Vec<PythonRow> {
    vec![
        PythonRow {
            case: "the module the published WSGIPath default names",
            module_file: "application.py",
            callable: "application",
            wsgi_path: None,
        },
        PythonRow {
            case: "a module the configuration names at the bundle's root",
            module_file: "app.py",
            callable: "application",
            wsgi_path: Some("app.py"),
        },
        PythonRow {
            case: "a module the configuration names, inside a package of the bundle",
            module_file: "project/wsgi.py",
            callable: "application",
            wsgi_path: Some("project/wsgi.py"),
        },
        PythonRow {
            case: "the module:callable form the platform's server takes",
            module_file: "server.py",
            callable: "handler",
            wsgi_path: Some("server:handler"),
        },
    ]
}

/// A Python WSGI application: it logs [`LOG_MARKER`] while being imported — which
/// is what an Elastic Beanstalk Python platform captures into the instance's
/// standard-output log — and answers every request with the environment property
/// it was configured with and the target it was asked for, read off the WSGI
/// environment it was handed.
fn python_application(callable: &str) -> Vec<u8> {
    format!(
        "import os\n\
         \n\
         print('{LOG_MARKER}')\n\
         \n\
         \n\
         def {callable}(environ, start_response):\n\
         \u{20}   start_response('200 OK', [('Content-Type', 'text/plain; charset=utf-8')])\n\
         \u{20}   target = environ['PATH_INFO']\n\
         \u{20}   query = environ.get('QUERY_STRING') or ''\n\
         \u{20}   if query:\n\
         \u{20}       target = target + '?' + query\n\
         \u{20}   body = (os.environ.get('{PROPERTY_NAME}') or '') + ' at ' + target\n\
         \u{20}   return [body.encode('utf-8')]\n",
    )
    .into_bytes()
}

/// Upload a bundle carrying `module_file` with the WSGI callable `callable` in it,
/// create the application and a version from it, and launch an environment on
/// `solution_stack` whose configuration is `settings` as
/// `(namespace, option, value)` triples.
///
/// The environment's name and its CNAME prefix are both `prefix`, so a suite that
/// launches more than one environment names each of them once.
async fn deployed_python_on(
    emulator: &Emulator,
    case: &str,
    prefix: &str,
    module_file: &str,
    callable: &str,
    solution_stack: &str,
    settings: &[(&str, &str, &str)],
) {
    let source = python_application(callable);
    let mut entries: Vec<(&str, &[u8])> = vec![(module_file, source.as_slice())];
    // A module inside a directory of the bundle is a module of that PACKAGE, so
    // the package's own file is in the deployment too, the way a Python project
    // that keeps its entry point in a package ships it.
    let package = module_file
        .rsplit_once('/')
        .map(|(directory, _)| format!("{directory}/__init__.py"));
    if let Some(package) = package.as_deref() {
        entries.push((package, b""));
    }
    emulator
        .put_object(
            BUNDLE_BUCKET,
            BUNDLE_KEY,
            &crate::application_version_processing::zip_of(&entries),
        )
        .await;
    ok(
        emulator,
        case,
        "CreateApplication",
        &[("ApplicationName", APPLICATION)],
    )
    .await;
    ok(
        emulator,
        case,
        "CreateApplicationVersion",
        &[
            ("ApplicationName", APPLICATION),
            ("VersionLabel", "v1"),
            ("SourceBundle.S3Bucket", BUNDLE_BUCKET),
            ("SourceBundle.S3Key", BUNDLE_KEY),
        ],
    )
    .await;
    // The configuration goes on the wire as the awsQuery list `OptionSettings`
    // is, one member per setting.
    let members: Vec<(String, String)> = settings
        .iter()
        .enumerate()
        .flat_map(|(index, (namespace, option, value))| {
            let member = index + 1;
            [
                (
                    format!("OptionSettings.member.{member}.Namespace"),
                    (*namespace).to_string(),
                ),
                (
                    format!("OptionSettings.member.{member}.OptionName"),
                    (*option).to_string(),
                ),
                (
                    format!("OptionSettings.member.{member}.Value"),
                    (*value).to_string(),
                ),
            ]
        })
        .collect();
    let mut params: Vec<(&str, &str)> = vec![
        ("ApplicationName", APPLICATION),
        ("EnvironmentName", prefix),
        ("CNAMEPrefix", prefix),
        ("SolutionStackName", solution_stack),
        ("VersionLabel", "v1"),
    ];
    params.extend(
        members
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str())),
    );
    ok(emulator, case, "CreateEnvironment", &params).await;
    // The launch the clock owes is waited for by LOOKING AGAIN until the
    // environment reports it is ready to serve, never by waiting a fixed time.
    wait_for_status(emulator, case, prefix, eb::state::STATUS_READY).await;
}

/// Upload a bundle carrying the Python module `row` names, create the application
/// and a version from it, and launch an environment deploying that version with
/// the environment property and the `WSGIPath` the row configures.
async fn deployed_python(emulator: &Emulator, row: &PythonRow, prefix: &str) {
    let mut settings: Vec<(&str, &str, &str)> = vec![(
        eb::state::NAMESPACE_APPLICATION_ENVIRONMENT,
        PROPERTY_NAME,
        PROPERTY_VALUE,
    )];
    if let Some(wsgi_path) = row.wsgi_path {
        settings.push((
            eb::state::NAMESPACE_PYTHON_CONTAINER,
            eb::state::OPTION_WSGI_PATH,
            wsgi_path,
        ));
    }
    deployed_python_on(
        emulator,
        row.case,
        prefix,
        row.module_file,
        row.callable,
        // A Python platform, because the platform an environment runs on is what
        // decides the engine its deployment is started in.
        &solution_stack_of(eb::state::PLATFORM_CATEGORY_PYTHON),
        &settings,
    )
    .await;
}

/// RULE: an environment whose bundle carries a PYTHON entry point runs it in the
/// Python interpreter this emulator embeds — the module the platform's own
/// `WSGIPath` names, else the module its published default names — so the address
/// the environment publishes is served by what that application's WSGI callable
/// answers, computed from the request it was handed.
#[tokio::test]
async fn a_python_bundle_is_served_by_its_wsgi_application() {
    for row in python_rows() {
        let case = row.case;
        let emulator = Emulator::with_object_storage();
        deployed_python(&emulator, &row, "deployed-env-1").await;

        // One row per target the client asks for, including a query string, which
        // WSGI carries beside the path.
        for path in ["/", "/status", "/items?page=2"] {
            let url = dialled(&emulator, "deployed-env-1", path);
            let served = emulator.get_url(&url).await;
            assert_eq!(
                served.status, 200,
                "case {case:?} {path:?}: expected 200 from {url}, got {} and body {}",
                served.status, served.body
            );
            assert!(
                served.body.contains(PROPERTY_VALUE),
                "case {case:?} {path:?}: the answer must carry what the application computed \
                 from its own environment, got {:?}",
                served.body
            );
            assert!(
                served.body.contains(&format!("at {path}")),
                "case {case:?} {path:?}: the application must answer from the request it was \
                 handed, got {:?}",
                served.body
            );
        }
    }
}

/// RULE: the compiled environment information carries a Python application's own
/// log lines, byte for byte, beside this service's activity log — the output the
/// platform captures into the instance's standard-output log file is the output
/// the deployed code really wrote.
#[tokio::test]
async fn the_compiled_information_carries_a_python_applications_log() {
    let row = PythonRow {
        case: "a Python application that logs while starting",
        module_file: "application.py",
        callable: "application",
        wsgi_path: None,
    };
    let case = row.case;
    let emulator = Emulator::with_object_storage();
    deployed_python(&emulator, &row, "deployed-env-1").await;

    ok(
        &emulator,
        case,
        "RequestEnvironmentInfo",
        &[
            ("EnvironmentName", "deployed-env-1"),
            ("InfoType", eb::state::ENVIRONMENT_INFO_TYPE_TAIL),
        ],
    )
    .await;
    // The compile the clock owes is waited for by LOOKING AGAIN until the
    // information is there, never by waiting a fixed time.
    let retrieved = retrieved_once_compiled(&emulator, case, "deployed-env-1").await;
    let compiled = entries(&retrieved);
    let entry = compiled
        .first()
        .unwrap_or_else(|| panic!("case {case:?}: expected an entry, got {compiled:?}"));
    let downloaded = emulator.get_url(&entry.message).await;
    assert_eq!(
        downloaded.status, 200,
        "case {case:?}: the compiled file must download, got {} and body {}",
        downloaded.status, downloaded.body
    );
    assert!(
        downloaded.body.contains(LOG_MARKER),
        "case {case:?}: the application's own log line ({LOG_MARKER:?}) must be carried, \
         got {:?}",
        downloaded.body
    );
    assert!(
        downloaded.body.contains(eb::state::APPLICATION_STDOUT_LOG),
        "case {case:?}: the application's log file must be named, got {:?}",
        downloaded.body
    );
}

/// Look again until the environment information a `RequestEnvironmentInfo`
/// started is there to retrieve — the CONDITION the caller waits on, never a
/// fixed wait for the clock — or until the harness's generous status deadline has
/// passed.
async fn retrieved_once_compiled(emulator: &Emulator, case: &str, environment: &str) -> Answer {
    let started = std::time::Instant::now();
    loop {
        let retrieved = ok(
            emulator,
            case,
            "RetrieveEnvironmentInfo",
            &[
                ("EnvironmentName", environment),
                ("InfoType", eb::state::ENVIRONMENT_INFO_TYPE_TAIL),
            ],
        )
        .await;
        if !entries(&retrieved).is_empty() {
            return retrieved;
        }
        assert!(
            started.elapsed() < STATUS_DEADLINE,
            "case {case:?}: the environment information was never compiled, last body {}",
            retrieved.body
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// RULE: the name an environment publishes is one a client really DIALS, as it
/// stands: the socket this emulator serves the environment plane on speaks
/// HTTP/1.1, and a plain HTTP request stating that name — the host
/// `EnvironmentDescription$CNAME` publishes, with no credential of any kind — is
/// answered by the deployed application.
///
/// The name is sent as the request's `Host`, which is what a client that resolved
/// it does, and the connection is opened to the loopback interface the core binds
/// its listeners on — the interface a resolver answers a name under the loopback
/// zone with (RFC 6761). What is under test is that the plane resolves the
/// environment out of the published name the request STATES.
#[tokio::test]
async fn the_published_address_answers_a_plain_http_request() {
    let row = PythonRow {
        case: "a Python application dialled at the published address",
        module_file: "application.py",
        callable: "application",
        wsgi_path: None,
    };
    let case = row.case;
    let emulator = Emulator::with_object_storage();
    deployed_python(&emulator, &row, "deployed-env-1").await;

    let name = published_address(&emulator, "deployed-env-1");
    let authority = emulator.plane_authority();
    let port: u16 = authority
        .rsplit_once(':')
        .and_then(|(_host, port)| port.parse().ok())
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected the plane to answer on a port, got {authority:?}")
        });
    let request =
        format!("GET / HTTP/1.1\r\nHost: {name}\r\nAccept: */*\r\nConnection: close\r\n\r\n");
    let mut connection = tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port))
        .await
        .unwrap_or_else(|error| {
            panic!("case {case:?}: expected {name} to be dialable, got {error}")
        });
    tokio::io::AsyncWriteExt::write_all(&mut connection, request.as_bytes())
        .await
        .unwrap_or_else(|error| panic!("case {case:?}: the request must be sent, got {error}"));
    let mut answered = Vec::new();
    tokio::io::AsyncReadExt::read_to_end(&mut connection, &mut answered)
        .await
        .unwrap_or_else(|error| panic!("case {case:?}: the answer must be read, got {error}"));
    let answered = String::from_utf8_lossy(&answered).into_owned();
    assert!(
        answered.starts_with("HTTP/1.1 200"),
        "case {case:?}: expected the application's own answer, got {answered:?}"
    );
    assert!(
        answered.contains(PROPERTY_VALUE),
        "case {case:?}: expected what the application computed, got {answered:?}"
    );
}

/// How long a published log group and a delivered event are given to be readable
/// in CloudWatch Logs, and how long to wait between looks.
///
/// Generous, and the suites wait for the GROUP and the EVENTS rather than for any
/// duration: they fail only where those never arrive at all.
const DELIVERY_DEADLINE: std::time::Duration = std::time::Duration::from_secs(60);
/// See [`DELIVERY_DEADLINE`].
const DELIVERY_POLL: std::time::Duration = std::time::Duration::from_millis(20);

/// One environment whose configuration streams its instance logs, and what
/// CloudWatch Logs must then hold for it.
struct StreamingRow {
    case: &'static str,
    /// The platform category of the solution stack the environment runs on,
    /// which is what decides WHICH on-instance files it streams.
    platform_category: &'static str,
    /// The `RetentionInDays` the configuration states, or `None` for one that
    /// states none and leaves the published default in force.
    retention: Option<&'static str>,
    /// The retention the published groups must then be kept for.
    expected_retention: i64,
    /// An on-instance file this platform streams besides the application's own
    /// output, which must therefore have a group of its own.
    streamed: &'static str,
    /// An on-instance file only ANOTHER platform's log task writes, which must
    /// have NO group at all.
    not_streamed: &'static str,
}

/// One row per platform family, retention and streamed/not-streamed pair the rule
/// generalises over.
fn streaming_rows() -> Vec<StreamingRow> {
    vec![
        StreamingRow {
            case: "a Python platform, with a retention of its own",
            platform_category: "Python",
            retention: Some("14"),
            expected_retention: 14,
            streamed: "/var/log/nginx/access.log",
            not_streamed: "/var/log/puma/puma.log",
        },
        StreamingRow {
            case: "a Ruby platform, leaving the published retention in force",
            platform_category: "Ruby",
            retention: None,
            expected_retention: 7,
            streamed: "/var/log/puma/puma.log",
            not_streamed: "/var/log/tomcat/catalina.out",
        },
        StreamingRow {
            case: "a Docker platform, at the longest published retention",
            platform_category: "Docker",
            retention: Some("3653"),
            expected_retention: 3653,
            streamed: "/var/log/eb-docker/containers/eb-current-app/stdouterr.log",
            not_streamed: "/var/log/puma/puma.log",
        },
    ]
}

/// The `SolutionStackName` of a platform version in `category`, taken from the
/// service's OWN published catalogue so a row cannot name a platform this
/// emulator does not offer.
///
/// `pub(crate)` so the other suites of this one binary read the same catalogue
/// rather than spelling a platform name of their own.
pub(crate) fn solution_stack_of(category: &str) -> String {
    eb::state::SOLUTION_STACKS
        .iter()
        .find(|stack| stack.platform_category == category)
        .unwrap_or_else(|| panic!("the catalogue publishes a {category} platform"))
        .name()
}

/// One CloudWatch Logs call (`awsJson1_1`), as a client makes it, answering its
/// parsed body.
///
/// The emulator under test has that service mounted beside Elastic Beanstalk on
/// one engine ([`Emulator::with_object_storage_and_logs`]), so what these suites
/// read is the store the delivery really wrote into. `pub(crate)` so the other
/// suites of this one binary read the delivered logs the same way.
pub(crate) async fn logs(emulator: &Emulator, case: &str, operation: &str, body: serde_json::Value) -> serde_json::Value {
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential={ACCOUNT_KEY_ID}/20261021/{TEST_REGION}/logs/aws4_request, \
         SignedHeaders=host, Signature=0123456789abcdef"
    );
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://logs.{TEST_REGION}.amazonaws.com/"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("Logs_20140328.{operation}"))
        .header("authorization", authorization)
        .body(bytes::Bytes::from(body.to_string()))
        .expect("a well-formed request");
    let answer = emulator.dispatch(request).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected 200 from {operation}, got {} and body {}",
        answer.status, answer.body
    );
    serde_json::from_str(&answer.body).unwrap_or(serde_json::Value::Null)
}

/// Every log group under `prefix` CloudWatch Logs holds, as
/// `(name, retentionInDays)`.
async fn log_groups(emulator: &Emulator, case: &str, prefix: &str) -> Vec<(String, Option<i64>)> {
    let answered = logs(
        emulator,
        case,
        "DescribeLogGroups",
        serde_json::json!({ "logGroupNamePrefix": prefix }),
    )
    .await;
    answered
        .get("logGroups")
        .and_then(serde_json::Value::as_array)
        .map(|groups| {
            groups
                .iter()
                .filter_map(|group| {
                    let name = group.get("logGroupName")?.as_str()?.to_string();
                    Some((name, group.get("retentionInDays").and_then(|days| days.as_i64())))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Wait until CloudWatch Logs holds at least `expected` groups under `prefix`,
/// and answer what it then holds.
async fn log_groups_once_published(
    emulator: &Emulator,
    case: &str,
    prefix: &str,
    expected: usize,
) -> Vec<(String, Option<i64>)> {
    let deadline = std::time::Instant::now() + DELIVERY_DEADLINE;
    loop {
        let held = log_groups(emulator, case, prefix).await;
        if held.len() >= expected || std::time::Instant::now() >= deadline {
            return held;
        }
        tokio::time::sleep(DELIVERY_POLL).await;
    }
}

/// Every event of the log group `log_group`, as `(stream name, message)` — read
/// through CloudWatch Logs' own reader of a group, which is where a client of the
/// streamed logs reads them.
async fn log_events(emulator: &Emulator, case: &str, log_group: &str) -> Vec<(String, String)> {
    let answered = logs(
        emulator,
        case,
        "FilterLogEvents",
        serde_json::json!({ "logGroupName": log_group }),
    )
    .await;
    answered
        .get("events")
        .and_then(serde_json::Value::as_array)
        .map(|events| {
            events
                .iter()
                .filter_map(|event| {
                    Some((
                        event.get("logStreamName")?.as_str()?.to_string(),
                        event.get("message")?.as_str()?.to_string(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Wait until that group carries at least one event, and answer what it carries.
async fn log_events_once_delivered(
    emulator: &Emulator,
    case: &str,
    log_group: &str,
) -> Vec<(String, String)> {
    let deadline = std::time::Instant::now() + DELIVERY_DEADLINE;
    loop {
        let carried = log_events(emulator, case, log_group).await;
        if !carried.is_empty() || std::time::Instant::now() >= deadline {
            return carried;
        }
        tokio::time::sleep(DELIVERY_POLL).await;
    }
}

/// The Amazon EC2 instances this service reports for `environment`, which is what
/// a stream of its streamed logs has to be attributable to.
async fn reported_instances(emulator: &Emulator, case: &str, environment: &str) -> Vec<String> {
    let answer = ok(
        emulator,
        case,
        "DescribeEnvironmentResources",
        &[("EnvironmentName", environment)],
    )
    .await;
    answer
        .values_at("EnvironmentResources/Instances/member/Id")
        .into_iter()
        .map(str::to_string)
        .collect()
}

/// RULE: an environment whose `aws:elasticbeanstalk:cloudwatch:logs` configuration
/// turns `StreamLogs` on publishes one CloudWatch Logs group per on-instance log
/// file its PLATFORM streams — named `/aws/elasticbeanstalk/<environment>` plus
/// that file's own path — keeps each of them for the `RetentionInDays` the
/// configuration states, and publishes NO group for a file only another platform's
/// log task writes.
#[tokio::test]
async fn streaming_publishes_a_group_per_file_the_platform_streams() {
    for row in streaming_rows() {
        let case = row.case;
        let environment = "logs-env-1";
        let emulator = Emulator::with_object_storage_and_logs();
        let mut settings: Vec<(&str, &str, &str)> = vec![(
            eb::state::NAMESPACE_CLOUDWATCH_LOGS,
            eb::state::OPTION_STREAM_LOGS,
            "true",
        )];
        if let Some(retention) = row.retention {
            settings.push((
                eb::state::NAMESPACE_CLOUDWATCH_LOGS,
                eb::state::OPTION_RETENTION_IN_DAYS,
                retention,
            ));
        }
        deployed_python_on(
            &emulator,
            case,
            environment,
            "application.py",
            "application",
            &solution_stack_of(row.platform_category),
            &settings,
        )
        .await;

        let prefix = format!("/aws/elasticbeanstalk/{environment}");
        let expected = eb::state::streamed_log_paths(Some(row.platform_category)).len();
        let held = log_groups_once_published(&emulator, case, &prefix, expected).await;
        let names: Vec<&str> = held.iter().map(|(name, _)| name.as_str()).collect();
        for path in eb::state::streamed_log_paths(Some(row.platform_category)) {
            let group = eb::state::log_group_name(environment, path);
            let retention = held
                .iter()
                .find(|(name, _)| *name == group)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: expected a group {group:?}, got {names:?}")
                })
                .1;
            assert_eq!(
                retention,
                Some(row.expected_retention),
                "case {case:?}: expected {group:?} kept for {} days, got {retention:?}",
                row.expected_retention
            );
        }
        // The file this platform streams beside the application's own output has
        // a group; the one only another platform writes has none.
        let streamed = eb::state::log_group_name(environment, row.streamed);
        assert!(
            names.contains(&streamed.as_str()),
            "case {case:?}: expected a group {streamed:?}, got {names:?}"
        );
        let not_streamed = eb::state::log_group_name(environment, row.not_streamed);
        assert!(
            !names.contains(&not_streamed.as_str()),
            "case {case:?}: expected no group {not_streamed:?}, got {names:?}"
        );
        // A client reading the one group that file would be in is told there is
        // none, rather than reading an empty page of a group nothing publishes.
        let absent = log_groups(&emulator, case, &not_streamed).await;
        assert!(
            absent.is_empty(),
            "case {case:?}: expected nothing under {not_streamed:?}, got {absent:?}"
        );
    }
}

/// RULE: the application's own standard output is DELIVERED into the group of the
/// file it goes to on the instance, one event per line it wrote, in a stream
/// attributable to one of the instances this service reports for the environment.
#[tokio::test]
async fn the_applications_output_is_delivered_to_its_own_group() {
    for row in streaming_rows() {
        let case = row.case;
        let environment = "logs-env-1";
        let emulator = Emulator::with_object_storage_and_logs();
        deployed_python_on(
            &emulator,
            case,
            environment,
            "application.py",
            "application",
            &solution_stack_of(row.platform_category),
            &[(
                eb::state::NAMESPACE_CLOUDWATCH_LOGS,
                eb::state::OPTION_STREAM_LOGS,
                "true",
            )],
        )
        .await;

        let group = eb::state::log_group_name(environment, eb::state::APPLICATION_STDOUT_LOG);
        let delivered = log_events_once_delivered(&emulator, case, &group).await;
        let messages: Vec<&str> = delivered
            .iter()
            .map(|(_, message)| message.as_str())
            .collect();
        assert!(
            messages.contains(&LOG_MARKER),
            "case {case:?}: expected the line the application printed ({LOG_MARKER:?}) in \
             {group:?}, got {messages:?}"
        );
        let instances = reported_instances(&emulator, case, environment).await;
        assert!(
            !instances.is_empty(),
            "case {case:?}: expected the environment to report its instances"
        );
        for (stream, message) in &delivered {
            assert!(
                instances.iter().any(|instance| stream.contains(instance)),
                "case {case:?}: expected the stream of {message:?} to name one of {instances:?}, \
                 got {stream:?}"
            );
        }
    }
}

/// RULE: streaming is OFF unless the configuration turns it on — the published
/// default — so an environment that does not set `StreamLogs`, and one that sets
/// it to `false`, has no log group of its own at all.
#[tokio::test]
async fn nothing_is_published_for_an_environment_that_streams_no_logs() {
    // (case, the StreamLogs the configuration states)
    let cases: &[(&str, Option<&str>)] = &[
        ("a configuration that states none", None),
        ("a configuration that turns it off", Some("false")),
    ];
    for (case, stream_logs) in cases {
        let environment = "logs-env-1";
        let emulator = Emulator::with_object_storage_and_logs();
        let mut settings: Vec<(&str, &str, &str)> = vec![(
            eb::state::NAMESPACE_CLOUDWATCH_LOGS,
            eb::state::OPTION_RETENTION_IN_DAYS,
            "30",
        )];
        if let Some(stream_logs) = stream_logs {
            settings.push((
                eb::state::NAMESPACE_CLOUDWATCH_LOGS,
                eb::state::OPTION_STREAM_LOGS,
                stream_logs,
            ));
        }
        deployed_python_on(
            &emulator,
            case,
            environment,
            "application.py",
            "application",
            &solution_stack_of(eb::state::PLATFORM_CATEGORY_PYTHON),
            &settings,
        )
        .await;

        let prefix = format!("/aws/elasticbeanstalk/{environment}");
        let held = log_groups(&emulator, case, &prefix).await;
        assert!(
            held.is_empty(),
            "case {case:?}: expected no log group under {prefix:?}, got {held:?}"
        );
        // And nothing is published by SERVING the application either: the address
        // answers, and the environment still streams nothing.
        let served = emulator
            .get_url(&dialled(&emulator, environment, "/"))
            .await;
        assert_eq!(
            served.status, 200,
            "case {case:?}: expected the address to be served, got {} and body {}",
            served.status, served.body
        );
        let after = log_groups(&emulator, case, &prefix).await;
        assert!(
            after.is_empty(),
            "case {case:?}: expected no log group after a request, got {after:?}"
        );
    }
}

/// RULE: an UPDATE that turns streaming on publishes the groups the configuration
/// it deploys calls for, exactly as a launch that turns it on does — the groups
/// exist from the call that turns the log on.
#[tokio::test]
async fn an_update_that_turns_streaming_on_publishes_the_groups() {
    let case = "an update that turns streaming on";
    let environment = "logs-env-1";
    let emulator = Emulator::with_object_storage_and_logs();
    deployed_python_on(
        &emulator,
        case,
        environment,
        "application.py",
        "application",
        &solution_stack_of(eb::state::PLATFORM_CATEGORY_PYTHON),
        &[],
    )
    .await;
    let prefix = format!("/aws/elasticbeanstalk/{environment}");
    let before = log_groups(&emulator, case, &prefix).await;
    assert!(
        before.is_empty(),
        "case {case:?}: expected no group before the update, got {before:?}"
    );

    ok(
        &emulator,
        case,
        "UpdateEnvironment",
        &[
            ("EnvironmentName", environment),
            (
                "OptionSettings.member.1.Namespace",
                eb::state::NAMESPACE_CLOUDWATCH_LOGS,
            ),
            (
                "OptionSettings.member.1.OptionName",
                eb::state::OPTION_STREAM_LOGS,
            ),
            ("OptionSettings.member.1.Value", "true"),
            (
                "OptionSettings.member.2.Namespace",
                eb::state::NAMESPACE_CLOUDWATCH_LOGS,
            ),
            (
                "OptionSettings.member.2.OptionName",
                eb::state::OPTION_RETENTION_IN_DAYS,
            ),
            ("OptionSettings.member.2.Value", "90"),
        ],
    )
    .await;

    let expected = eb::state::streamed_log_paths(Some(eb::state::PLATFORM_CATEGORY_PYTHON)).len();
    let held = log_groups_once_published(&emulator, case, &prefix, expected).await;
    let group = eb::state::log_group_name(environment, eb::state::APPLICATION_STDOUT_LOG);
    let retention = held.iter().find(|(name, _)| *name == group).map(|(_, days)| *days);
    assert_eq!(
        retention,
        Some(Some(90)),
        "case {case:?}: expected {group:?} published and kept for 90 days, got {held:?}"
    );
}
