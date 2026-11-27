// SPDX-License-Identifier: CC0-1.0

//! What a managed `nodejs*` runtime's STANDARD LIBRARY answers a handler, over the wire: a
//! function whose module body imports the library's modules runs, and the console that
//! library installs carries the whole surface a program calls on it.
//!
//! RULE: a managed node runtime is a promise about an interpreter AND about the standard
//! library that comes with it, so every module of that library RESOLVES when a handler's
//! module body imports it — only the CALL that needs a host capability this runtime has none
//! of is refused, never the import — and the console the library installs is one object
//! carrying every method node documents, whether a program reads the global or builds a
//! `Console` of its own over its own streams.
//!
//! That is what an import failure costs: a module body is evaluated before the handler is
//! ever entered, so one unresolvable `require` means the handler never runs and whatever it
//! was going to answer is never sent. The shape under test is exactly the one a bundled
//! provider function ships — `const { Console } = require('console')` and then `new
//! Console({ stdout: process.stdout })` at module scope, beside the `child_process` import
//! its SDK installer uses — which is why a CloudFormation custom resource backed by such a
//! provider never reached a terminal state while one of those modules was absent.
//!
//! The rows differ along what the rule generalises over: WHICH module is imported, and what
//! the module is for — the console the library installs, a view of something the execution
//! environment already holds (its process object, its terminals, the submodules of modules
//! already here), and a capability this runtime has none of (an HTTP/2 session, a datagram
//! socket, a forked worker, an inspector, a REPL, a WebAssembly system interface), where the
//! import still has to load and the call is refused with the number a host answers for a
//! system call that is not there. One function's module body imports every one of them, so a
//! row that would fail to import fails this suite before any assertion is reached.
//!
//! The suite RUNS a handler, so it takes the target's one invocation stream
//! ([`crate::invoke_request_id::one_invocation_stream_at_a_time`]).

use std::sync::Arc;

use base64::Engine as _;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::event_source_mapping_role_access::given_role;
use crate::event_source_mappings::{await_function_active, call, call_signed_by, router_on};

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// The `Runtime` the function carries: a managed node runtime, which is what promises the
/// standard library under test.
const RUNTIME: &str = "nodejs22.x";

/// The two records the handler writes to the invocation's log: one through a `Console` of its
/// own built over the process's output stream — the logger a bundled provider builds — and one
/// through the global console the runtime installs.
const OWN_CONSOLE_RECORD: &str = "a record of the handler's own console";
const GLOBAL_CONSOLE_RECORD: &str = "a record of the runtime's console";

/// The handler: its MODULE BODY imports every module this suite is about, builds a logger
/// over the process's own output stream the way a bundled provider does, and then reports
/// what each of them answered — one entry per case, as the text that case's rule states.
///
/// Nothing here is synthesised: every entry is read off the module the body imported.
const HANDLER_SOURCE: &str = r#"
'use strict';

// The module body a bundled custom-resource provider has: the child-process entry point its
// SDK installer uses, and a console of its own over the process's output stream.
const { execSync } = require('child_process');
const { Console } = require('console');
const processModule = require('process');
const tty = require('tty');
const types = require('util/types');
const posix = require('path/posix');
const sys = require('sys');
const http2 = require('http2');
const dgram = require('dgram');
const cluster = require('cluster');
const inspector = require('inspector');
const inspectorPromises = require('inspector/promises');
const repl = require('repl');
const { WASI } = require('wasi');
const traceEvents = require('trace_events');

const logger = new Console({ stdout: process.stdout, stderr: process.stderr });

// The methods node documents on a console, every one of which a program may call.
const CONSOLE_METHODS = [
    'log', 'info', 'debug', 'dir', 'dirxml', 'warn', 'error', 'trace', 'assert', 'count',
    'countReset', 'group', 'groupCollapsed', 'groupEnd', 'time', 'timeLog', 'timeEnd',
    'table', 'clear', 'timeStamp', 'profile', 'profileEnd',
];

// The code a refused call reports, or what happened instead when it was not refused.
function refusalOf(call) {
    try {
        call();
    } catch (error) {
        return String(error.code);
    }
    return 'not-refused';
}

// Every record a console of this program's own writes, in the order it wrote them.
function ownRecords() {
    const written = [];
    const sink = { write: (text) => { written.push(text); return true; } };
    const own = new Console({ stdout: sink });
    own.log('%s=%d', 'seeded', 7);
    own.group('inside a group');
    own.log('indented');
    own.groupEnd();
    own.log('outside it');
    own.count('rows');
    own.count('rows');
    own.time('span');
    own.timeEnd('span');
    own.table([{ id: 'a' }, { id: 'b' }]);
    own.assert(false, 'not so');
    return written.map((line) => line.replace(/\n$/, ''));
}

exports.handler = async () => {
    logger.log('%s', 'a record of the handler\'s own console');
    console.log('%s', 'a record of the runtime\'s console');

    const records = ownRecords();
    const promised = await new inspectorPromises.Session().post('Runtime.evaluate')
        .then(() => 'not-refused', (error) => String(error.code));
    const tracing = traceEvents.createTracing({ categories: ['node.perf'] });
    tracing.enable();

    return {
        'the console module is the global console': String(require('console') === console),
        'the global console carries every documented method':
            CONSOLE_METHODS.filter((name) => typeof console[name] !== 'function').join(','),
        'a console of the program\'s own formats its records': records[0],
        'a group indents the records written inside it': records[1] + '/' + records[2],
        'a group ends where it was ended': records[3],
        'a count reports the calls under its label': records[4] + '/' + records[5],
        'a timed span is reported under its label':
            records[6].replace(/[0-9]+(\.[0-9]+)?/, '<elapsed>'),
        'a table is drawn with a column per member': records[7],
        'a failed assertion is reported and not thrown': records[8],
        'the SDK installer\'s own entry point is refused':
            refusalOf(() => execSync('npm install')),
        'the process module is the global process': String(processModule === process),
        'no descriptor of this environment is a terminal': String(tty.isatty(1)),
        'a terminal stream cannot be built': refusalOf(() => new tty.WriteStream(1)),
        'util/types classifies a value':
            String(types.isDate(new Date()) && !types.isDate(7)),
        'path/posix joins a path': posix.join('seeded', '..', 'answers', 'one.json'),
        'sys is the util module': String(sys === require('util')),
        'the HTTP/2 pseudo-header names are stated': http2.constants.HTTP2_HEADER_PATH,
        'the HTTP/2 default settings are stated':
            String(http2.getDefaultSettings().initialWindowSize),
        'packing HTTP/2 settings is refused': refusalOf(() => http2.getPackedSettings({})),
        'a datagram socket has no address': refusalOf(() => dgram.createSocket('udp4').address()),
        'this process is the cluster primary': String(cluster.isPrimary),
        'forking a worker is refused': refusalOf(() => cluster.fork()),
        'no inspector is attached': String(inspector.url()),
        'opening an inspector is refused': refusalOf(() => inspector.open()),
        'a promised inspector post is rejected': promised,
        'starting a REPL is refused': refusalOf(() => repl.start()),
        'the REPL writer inspects a value': repl.writer('seeded'),
        'a WASI instance cannot be built': refusalOf(() => new WASI({})),
        'an enabled tracing object reports its categories':
            traceEvents.getEnabledCategories(),
    };
};
"#;

/// Lambda, IAM and the token service over ONE shared engine, as `core/router/src/lib.rs`
/// mounts them: the function runs as a role IAM really holds.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = router_on(&engine);
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router
}

/// Create the function `name` out of [`HANDLER_SOURCE`], running as `role_arn`.
async fn given_function(router: &MultiServiceRouter, case: &str, name: &str, role_arn: &str) {
    let created = call(
        router,
        "POST",
        FUNCTIONS_URI,
        &json!({
            "FunctionName": name,
            "Role": role_arn,
            "Runtime": RUNTIME,
            "Handler": "index.handler",
            "Timeout": 60,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD
                    .encode(crate::node_invoke_run::package(&[("index.js", HANDLER_SOURCE)])),
            },
        }),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "case {case:?}: expected CreateFunction to answer 201, got {} {}",
        created.status,
        created.text()
    );
    await_function_active(router, case, name).await;
}

/// One row: the case the handler reported under, and what the rule says it reports.
struct Row {
    case: &'static str,
    answers: &'static str,
}

/// The table the drawn `console.table` of two rows carrying one member must be: a column for
/// the row labels, a column for the member, and every cell padded to its column's width.
const DRAWN_TABLE: &str = "\
┌─────────┬─────┐
│ (index) │ id  │
├─────────┼─────┤
│ 0       │ 'a' │
│ 1       │ 'b' │
└─────────┴─────┘";

fn rows() -> Vec<Row> {
    vec![
        // The console the standard library installs: ONE object, carrying everything node
        // documents on it.
        Row {
            case: "the console module is the global console",
            answers: "true",
        },
        Row {
            case: "the global console carries every documented method",
            answers: "",
        },
        Row {
            case: "a console of the program's own formats its records",
            answers: "seeded=7",
        },
        Row {
            case: "a group indents the records written inside it",
            answers: "inside a group/  indented",
        },
        Row {
            case: "a group ends where it was ended",
            answers: "outside it",
        },
        Row {
            case: "a count reports the calls under its label",
            answers: "rows: 1/rows: 2",
        },
        Row {
            case: "a timed span is reported under its label",
            answers: "span: <elapsed>ms",
        },
        Row {
            case: "a table is drawn with a column per member",
            answers: DRAWN_TABLE,
        },
        Row {
            case: "a failed assertion is reported and not thrown",
            answers: "Assertion failed: not so",
        },
        // The module whose capability this runtime has none of that a bundled provider
        // already imports, which is the refusal this suite's shape was built around.
        Row {
            case: "the SDK installer's own entry point is refused",
            answers: "ENOSYS",
        },
        // The views of what the execution environment already holds.
        Row {
            case: "the process module is the global process",
            answers: "true",
        },
        Row {
            case: "no descriptor of this environment is a terminal",
            answers: "false",
        },
        Row {
            case: "a terminal stream cannot be built",
            answers: "ENOTTY",
        },
        Row {
            case: "util/types classifies a value",
            answers: "true",
        },
        Row {
            case: "path/posix joins a path",
            answers: "answers/one.json",
        },
        Row {
            case: "sys is the util module",
            answers: "true",
        },
        // The capabilities this runtime has none of: the names are stated, the computation is
        // performed, and the call that needs the capability is refused.
        Row {
            case: "the HTTP/2 pseudo-header names are stated",
            answers: ":path",
        },
        Row {
            case: "the HTTP/2 default settings are stated",
            answers: "65535",
        },
        Row {
            case: "packing HTTP/2 settings is refused",
            answers: "ENOSYS",
        },
        Row {
            case: "a datagram socket has no address",
            answers: "ENOSYS",
        },
        Row {
            case: "this process is the cluster primary",
            answers: "true",
        },
        Row {
            case: "forking a worker is refused",
            answers: "ENOSYS",
        },
        Row {
            case: "no inspector is attached",
            answers: "undefined",
        },
        Row {
            case: "opening an inspector is refused",
            answers: "ENOSYS",
        },
        Row {
            case: "a promised inspector post is rejected",
            answers: "ENOSYS",
        },
        Row {
            case: "starting a REPL is refused",
            answers: "ENOSYS",
        },
        Row {
            case: "the REPL writer inspects a value",
            answers: "'seeded'",
        },
        Row {
            case: "a WASI instance cannot be built",
            answers: "ENOSYS",
        },
        Row {
            case: "an enabled tracing object reports its categories",
            answers: "node.perf",
        },
    ]
}

/// A handler whose module body imports the standard library runs, and every module it
/// imported answers what the rule says it answers.
#[tokio::test]
async fn a_handler_importing_the_standard_library_runs_and_each_module_answers() {
    let _stream = crate::invoke_request_id::one_invocation_stream_at_a_time().await;
    let case = "a module body importing the standard library";
    let router = emulator();
    let role = given_role(&router, case, "node-standard-library-role", None).await;
    let function = "node-standard-library-reader";
    given_function(&router, case, function, &role).await;

    let answered = call_signed_by(
        &router,
        None,
        "POST",
        &format!("{FUNCTIONS_URI}/{function}/invocations"),
        &json!({}),
        &[("x-amz-log-type", "Tail")],
    )
    .await;
    assert_eq!(
        answered.status,
        200,
        "case {case:?}: expected HTTP 200 from Invoke, got {} {}",
        answered.status,
        answered.text()
    );
    // A module body that cannot be imported is reported here, before any row is reached: the
    // handler is never entered, which is exactly what leaves a caller waiting for an answer
    // that never comes.
    assert_eq!(
        answered.header("x-amz-function-error"),
        None,
        "case {case:?}: the handler's module body must import and run, got {}",
        answered.text()
    );

    let reported = answered.json();
    for row in rows() {
        assert_eq!(
            reported.get(row.case).and_then(Value::as_str),
            Some(row.answers),
            "case {:?}: expected {:?}, got {:?}",
            row.case,
            row.answers,
            reported.get(row.case)
        );
    }

    // The records both consoles wrote reach the invocation's own log, which is where a client
    // reads what a provider function logged.
    let log = answered
        .header("x-amz-log-result")
        .map(|encoded| {
            String::from_utf8_lossy(
                &base64::engine::general_purpose::STANDARD
                    .decode(encoded)
                    .unwrap_or_default(),
            )
            .to_string()
        })
        .unwrap_or_default();
    for record in [OWN_CONSOLE_RECORD, GLOBAL_CONSOLE_RECORD] {
        assert!(
            log.contains(record),
            "case {case:?}: expected the invocation's log to carry {record:?}, got {log:?}"
        );
    }
}
