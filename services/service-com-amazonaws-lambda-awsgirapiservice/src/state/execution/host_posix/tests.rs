// SPDX-License-Identifier: CC0-1.0

//! The `posix` surface a run is given, driven through the INTERPRETER itself.
//!
//! The rule under test: the Python a Lambda function runs on presents the operating-system
//! surface the platform it reports itself to be presents — the CALLS `os` builds itself out
//! of, and the DATA it re-exports (the open flags, the access modes, the `stat_result` type)
//! — so a module built only out of the standard library imports cleanly and the handler's
//! own value is the answer. A name is absent only where the platform has none, and the names
//! that reach a host process or a host file are present and REFUSE.
//!
//! These runs are driven straight at [`super::super::python::run`], the way that module's
//! own tests drive theirs: no HTTP, no run slot and no concurrency, so the rule is pinned
//! here whatever load the rest of a test binary puts on the shared bounds. The crate's
//! `tests/all/python_runtime_os_surface.rs` drives the same tables end to end over the
//! router.

use std::collections::BTreeMap;
use std::time::Duration;

use super::super::package::PackageModule;
use super::super::python::{run, Budget, Completion, Finished, Run};

/// The module every package here holds.
const MODULE: &str = "lambda_function";

/// The handler every run below is configured with.
const HANDLER: &str = "lambda_function.lambda_handler";

/// What one run of these tables may spend: enough for a module that imports a good part of
/// the standard library, and well under the ceiling the service clamps a run to.
fn budget() -> Budget {
    Budget {
        wall_clock: Duration::from_secs(30),
        steps: 20_000_000,
        memory_bytes: 480 * 1024 * 1024,
        ephemeral_bytes: crate::state::execution::ephemeral_storage_bytes(
            crate::state::DEFAULT_EPHEMERAL_STORAGE_MB,
        ),
        log_bytes: 16 * 1024,
        response_bytes: 256 * 1024,
        recursion_limit: 2_000,
    }
}

/// Run `source` as the module `lambda_function` of a one-file package, on a thread with the
/// stack a run is given — the same way the service runs one.
fn ran(source: &str, event: &str) -> Finished {
    let modules = vec![PackageModule {
        path: format!("{MODULE}.py"),
        source: source.to_string(),
    }];
    let event = event.to_string();
    ::std::thread::Builder::new()
        .stack_size(crate::state::execution::RUN_STACK_BYTES)
        .spawn(move || {
            run(&Run {
                modules: &modules,
                handler: HANDLER,
                environment: &BTreeMap::new(),
                architecture: crate::state::DEFAULT_ARCHITECTURE,
                event: Some(event.as_str()),
                request_id: "11111111-2222-3333-4444-555555555555",
                invoked_function_arn: "arn:aws:lambda:us-west-2:000000000000:function:probe",
                client_context: None,
                tenant_id: None,
                // These runs report on the host surface, so they are the runs of no caller's
                // credential and carry no Cognito identity.
                cognito_identity: None,
                budget: budget(),
                // These runs are the interpreter's own surface: no emulator stands behind
                // them, so there is no SDK seam either.
                aws: None,
                // Nor a durable execution: nothing here is the run of one.
                durable: None,
                // Nor a mounted file system: what these runs read is the environment's own.
                file_systems: &[],
            })
        })
        .expect("a run thread")
        .join()
        .expect("the run thread ended")
}

/// The JSON the handler returned, or a panic naming what the run came to instead.
fn returned(finished: &Finished, case: &str) -> ::serde_json::Value {
    match &finished.completion {
        Completion::Returned(payload) => ::serde_json::from_str(payload)
            .unwrap_or_else(|_| panic!("case {case}: expected JSON, got {payload:?}")),
        other => panic!(
            "case {case}: expected the handler's own value, got {other:?} (log {:?})",
            finished.log
        ),
    }
}

/// A module built only out of the standard library imports cleanly, and the value its
/// handler computed is the answer.
///
/// The rows differ along WHICH part of the surface the module reaches as it is imported (the
/// fork registration the standard library performs, the open flags it combines, the result
/// type it names) and along which standard-library module reaches it, because the surface
/// `os` presents is a property of the runtime and not of any of those.
#[test]
fn a_module_built_out_of_the_standard_library_imports_and_runs() {
    // The case, the module source, and the members the answer must carry.
    let rows: &[(&str, &str, ::serde_json::Value)] = &[
        (
            // The shape the recorded in-VPC probe module has, minus the network it reaches:
            // the body imports `random`, which registers a reseed for the child of a fork as
            // it is imported.
            "a module whose body imports random and struct",
            "import random\n\
             import struct\n\
             def lambda_handler(event, context):\n    \
                 drawn = random.Random(event['seed']).randrange(1000)\n    \
                 return {'packed': list(struct.pack('!HH', 1, 258)),\n            \
                         'in_range': 0 <= drawn < 1000}\n",
            ::serde_json::json!({"packed": [0, 1, 1, 2], "in_range": true}),
        ),
        (
            // The minimal equivalent: the registration itself, at import time.
            "a module that registers a child callback as it is imported",
            "import os\n\
             os.register_at_fork(after_in_child=lambda: None)\n\
             def lambda_handler(event, context):\n    \
                 return {'registered': True}\n",
            ::serde_json::json!({"registered": true}),
        ),
        (
            "a module that registers every phase as it is imported",
            "import os\n\
             def before():\n    return None\n\
             def in_child():\n    return None\n\
             def in_parent():\n    return None\n\
             os.register_at_fork(before=before, after_in_child=in_child,\n                 \
                                 after_in_parent=in_parent)\n\
             def lambda_handler(event, context):\n    \
                 return {'answered': os.register_at_fork(before=before) is None,\n            \
                         'has_call': hasattr(os, 'register_at_fork'),\n            \
                         'has_fork': hasattr(os, 'fork')}\n",
            ::serde_json::json!({"answered": true, "has_call": true, "has_fork": true}),
        ),
        (
            "a module that imports logging, which registers all three phases",
            "import logging\n\
             def lambda_handler(event, context):\n    \
                 return {'level': logging.getLevelName(logging.INFO),\n            \
                         'named': logging.getLogger('probe').name}\n",
            ::serde_json::json!({"level": "INFO", "named": "probe"}),
        ),
        (
            // `glob` combines `os.O_RDONLY` with the directory flag as it is imported, which
            // is what reaches the open-flag and access-mode DATA.
            //
            // Source of the expected numbers: POSIX fixes the access modes and the three
            // open modes, so those are asserted as the numbers they are; the remaining open
            // flags are the platform's own bits, read from its C headers, so what the row
            // asserts of them is that each is one distinct bit rather than a placeholder.
            "a module that imports glob, which combines the open flags",
            "import glob\n\
             import os\n\
             def lambda_handler(event, context):\n    \
                 flags = (os.O_CREAT, os.O_EXCL, os.O_TRUNC, os.O_APPEND, os.O_NONBLOCK,\n                      \
                          os.O_DIRECTORY, os.O_NOFOLLOW, os.O_CLOEXEC)\n    \
                 return {'read_only': os.O_RDONLY,\n            \
                         'write_only': os.O_WRONLY,\n            \
                         'read_write': os.O_RDWR,\n            \
                         'access': [os.F_OK, os.X_OK, os.W_OK, os.R_OK],\n            \
                         'seek': [os.SEEK_SET, os.SEEK_CUR, os.SEEK_END],\n            \
                         'distinct_bits': len(set(flags)) == len(flags),\n            \
                         'single_bits': [f for f in flags if f <= 0 or f & (f - 1)],\n            \
                         'has_escape': hasattr(glob, 'escape')}\n",
            ::serde_json::json!({
                "read_only": 0,
                "write_only": 1,
                "read_write": 2,
                "access": [0, 1, 2, 4],
                "seek": [0, 1, 2],
                "distinct_bits": true,
                "single_bits": [],
                "has_escape": true,
            }),
        ),
        (
            // `shutil` reads `os.stat_result` as it is imported, and `pathlib` names it in
            // its own signatures, which is what reaches the result TYPE. The row reads the
            // type back and builds one, because a type that cannot hold a `stat` structure's
            // fields is not the type `os` re-exports.
            "a module that imports shutil and pathlib, which name the result type",
            "import os\n\
             import pathlib\n\
             import shutil\n\
             def lambda_handler(event, context):\n    \
                 fields = ('st_mode', 'st_ino', 'st_dev', 'st_nlink', 'st_uid', 'st_gid',\n                           \
                           'st_size', 'st_atime', 'st_mtime', 'st_ctime')\n    \
                 built = os.stat_result(range(10), {'st_blocks': 8})\n    \
                 return {'named': os.stat_result.__name__,\n            \
                         'positions': os.stat_result.n_sequence_fields,\n            \
                         'missing': [f for f in fields if not hasattr(os.stat_result, f)],\n            \
                         'sequence': issubclass(os.stat_result, tuple),\n            \
                         'read_back': [built.st_mode, built.st_size, built.st_ctime],\n            \
                         'beyond': built.st_blocks,\n            \
                         'unpacked': list(built),\n            \
                         'has_copy': hasattr(shutil, 'copystat'),\n            \
                         'root': str(pathlib.PurePosixPath('/var/task', 'index.py'))}\n",
            ::serde_json::json!({
                "named": "stat_result",
                "positions": 10,
                "missing": [],
                "sequence": true,
                "read_back": [0, 6, 9],
                "beyond": 8,
                "unpacked": [0, 1, 2, 3, 4, 5, 6, 7, 8, 9],
                "has_copy": true,
                "root": "/var/task/index.py",
            }),
        ),
    ];
    for (case, source, expected) in rows {
        let finished = ran(source, r#"{"seed": 7}"#);
        let answered = returned(&finished, case);
        let members = expected
            .as_object()
            .expect("every row of this table expects a document");
        for (member, value) in members {
            assert_eq!(
                answered.get(member),
                Some(value),
                "case {case}: expected {member:?} to be {value}, got {answered}"
            );
        }
    }
}

/// The handler that drives every ill-formed registration the table names and reports the
/// error each one raised.
const REFUSALS_SOURCE: &str = "import os\n\
     def once(case):\n    \
         if case == 'no phase at all':\n        \
             os.register_at_fork()\n    \
         elif case == 'a value that is not callable':\n        \
             os.register_at_fork(after_in_child=7)\n    \
         elif case == 'a phase given as None':\n        \
             os.register_at_fork(after_in_child=None)\n    \
         elif case == 'a callback passed positionally':\n        \
             os.register_at_fork(lambda: None)\n    \
         elif case == 'a phase the call does not have':\n        \
             os.register_at_fork(after_in_grandchild=lambda: None)\n    \
         elif case == 'a good phase beside a bad one':\n        \
             os.register_at_fork(before=lambda: None, after_in_child=7)\n\
     def lambda_handler(event, context):\n    \
         outcome = {}\n    \
         for case in event['cases']:\n        \
             try:\n            \
                 once(case)\n            \
                 outcome[case] = ''\n        \
             except BaseException as error:\n            \
                 outcome[case] = type(error).__name__\n    \
         return outcome\n";

/// The fork registration keeps the platform's own contract: the phases are keyword-only, at
/// least one has to be given, and each one has to be callable. One row per way a call breaks
/// it, and every one is refused as the `TypeError` the platform raises for it — so a module
/// that registers correctly imports, and one that does not is told so rather than quietly
/// accepted.
#[test]
fn an_ill_formed_fork_registration_is_refused_the_way_the_platform_refuses_it() {
    let cases: &[&str] = &[
        "no phase at all",
        "a value that is not callable",
        "a phase given as None",
        "a callback passed positionally",
        "a phase the call does not have",
        "a good phase beside a bad one",
    ];
    let event = ::serde_json::json!({"cases": cases}).to_string();
    let finished = ran(REFUSALS_SOURCE, &event);
    let answered = returned(&finished, "the refusal table");
    for case in cases {
        let raised = answered
            .get(*case)
            .and_then(::serde_json::Value::as_str)
            .unwrap_or("<not reported>");
        assert_eq!(
            raised, "TypeError",
            "case {case}: a call that breaks the contract is refused as a TypeError, got \
             {raised:?} in {answered}"
        );
    }
}

/// The handler of the catalog: it imports each module the event names and reports what each
/// import came to, so one run covers a group of the catalog and names every failure.
const IMPORT_PROBE_SOURCE: &str = "import importlib\n\
     def lambda_handler(event, context):\n    \
         outcome = {}\n    \
         for name in event['modules']:\n        \
             try:\n            \
                 importlib.import_module(name)\n            \
                 outcome[name] = ''\n        \
             except BaseException as error:\n            \
                 outcome[name] = '%s: %s' % (type(error).__name__, error)\n    \
         return outcome\n";

/// How many modules of the catalog one run is asked to import, so no single run has to hold
/// a large part of the standard library at once.
const CATALOG_GROUP: usize = 16;

/// The standard-library modules this runtime offers, which a handler must be able to import.
///
/// Source: the modules of the CPython standard library that are pure Python or are backed by
/// an extension this runtime registers (`super::super::python::ALLOWED_NATIVE_MODULES`),
/// read off the frozen standard library the interpreter ships. A module that needs an
/// extension this runtime registers no replacement for is not here — that is a missing
/// extension rather than this surface, and each is the subject of its own report.
const OFFERED_MODULES: &[&str] = &[
    "abc",
    "argparse",
    "array",
    "ast",
    "base64",
    "binascii",
    "bisect",
    "bz2",
    "calendar",
    "cmath",
    "codecs",
    "codeop",
    "collections",
    "collections.abc",
    "colorsys",
    "concurrent.futures",
    "configparser",
    "contextlib",
    "copy",
    "copyreg",
    "csv",
    "dataclasses",
    "datetime",
    "decimal",
    "difflib",
    "dis",
    "enum",
    "filecmp",
    "fileinput",
    "fnmatch",
    "fractions",
    "functools",
    "getopt",
    "gettext",
    "glob",
    "gzip",
    "hashlib",
    "heapq",
    "hmac",
    "html",
    "html.parser",
    "inspect",
    "io",
    "ipaddress",
    "itertools",
    "json",
    "keyword",
    "linecache",
    "logging",
    "lzma",
    "math",
    "mimetypes",
    "netrc",
    "numbers",
    "operator",
    "os",
    "os.path",
    "pathlib",
    "pickle",
    "posixpath",
    "pprint",
    "queue",
    "quopri",
    "random",
    "re",
    "reprlib",
    "sched",
    "secrets",
    "shlex",
    "shutil",
    "stat",
    "statistics",
    "string",
    "struct",
    "tempfile",
    "textwrap",
    "threading",
    "time",
    "timeit",
    "token",
    "tokenize",
    "traceback",
    "types",
    "typing",
    "unicodedata",
    "urllib.parse",
    "uuid",
    "warnings",
    "weakref",
    "zipfile",
    "zlib",
];

/// Every standard-library module this runtime offers imports cleanly — the generalisation of
/// the rule past the one module a report names, driven from inside a run so each import is
/// the one a handler performs.
#[test]
fn every_standard_library_module_this_runtime_offers_imports_cleanly() {
    for group in OFFERED_MODULES.chunks(CATALOG_GROUP) {
        let event = ::serde_json::json!({"modules": group}).to_string();
        let finished = ran(IMPORT_PROBE_SOURCE, &event);
        let answered = returned(&finished, "the catalog");
        for module in group {
            let outcome = answered
                .get(*module)
                .and_then(::serde_json::Value::as_str)
                .unwrap_or("<not reported>");
            assert_eq!(
                outcome, "",
                "case {module}: expected the module to import cleanly, got {outcome:?}"
            );
        }
    }
}

/// The surface being complete does not make it the HOST's: every call of it that would reach
/// a host process or a host file is PRESENT and REFUSES. One row per call the fork
/// registration and the open flags sit beside.
#[test]
fn the_completed_surface_still_reaches_no_host_process_or_file() {
    const SOURCE: &str = "import os\n\
         REACHES = (('fork', ()), ('forkpty', ()), ('wait', ()), ('kill', (1, 9)),\n                   \
                    ('system', ('echo hi',)), ('open', ('/etc/hostname', 0)),\n                   \
                    ('listdir', ('/',)), ('stat', ('/etc/hostname',)))\n\
         def lambda_handler(event, context):\n    \
             outcome = {}\n    \
             for call, arguments in REACHES:\n        \
                 try:\n            \
                     getattr(os, call)(*arguments)\n            \
                     outcome[call] = 'answered'\n        \
                 except BaseException as error:\n            \
                     outcome[call] = type(error).__name__\n    \
             return outcome\n";
    let finished = ran(SOURCE, "{}");
    let answered = returned(&finished, "the host surface");
    for call in [
        "fork", "forkpty", "wait", "kill", "system", "open", "listdir", "stat",
    ] {
        let outcome = answered
            .get(call)
            .and_then(::serde_json::Value::as_str)
            .unwrap_or("<not reported>");
        assert_eq!(
            outcome, "OSError",
            "case {call}: the call exists and refuses, so reaching the host raises OSError, \
             got {outcome:?} in {answered}"
        );
    }
}
