Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda.nodejs-child-process-exec-sync lambda.nodejs-native-addon-from-npm-loads lambda.nodejs-package-json-dependency-tree-imports lambda.nodejs-package-json-esm-dependency-with-exports lambda.nodejs-package-json-lodash-and-dayjs lambda.python-asyncio-run-completes lambda.python-ctypes-loads-libc lambda.python-lzma-and-bz2-round-trip lambda.python-sqlite3-in-memory-query lambda.python-subprocess-runs-a-command lambda.python-zoneinfo-resolves lambda.python-ssl-default-context-verifies lambda.python-threadpool-executor-runs lambda.python-runtime-reports-configured-version
Issue-Key: lambda.nodejs-child-process-exec-sync|_invoke_ok#3|AssertionError|assert err is None, f"the function failed: {err!r} {out!r}"
Issue-Key: lambda.nodejs-native-addon-from-npm-loads|_npm_package#2|Timeout|r = subprocess.run(
Issue-Key: lambda.nodejs-package-json-dependency-tree-imports|_npm_package#2|Timeout|r = subprocess.run(
Issue-Key: lambda.nodejs-package-json-esm-dependency-with-exports|_npm_package#2|Timeout|r = subprocess.run(
Issue-Key: lambda.nodejs-package-json-lodash-and-dayjs|_npm_package#2|Timeout|r = subprocess.run(
Issue-Key: lambda.python-asyncio-run-completes|_invoke_ok#3|AssertionError|assert err is None, f"the function failed: {err!r} {out!r}"
Issue-Key: lambda.python-ctypes-loads-libc|_invoke_ok#3|AssertionError|assert err is None, f"the function failed: {err!r} {out!r}"
Issue-Key: lambda.python-lzma-and-bz2-round-trip|_invoke_ok#4|AssertionError|assert err is None, f"the function failed: {err!r} {out!r}"
Issue-Key: lambda.python-sqlite3-in-memory-query|_invoke_ok#4|AssertionError|assert err is None, f"the function failed: {err!r} {out!r}"
Issue-Key: lambda.python-subprocess-runs-a-command|_invoke_ok#3|AssertionError|assert err is None, f"the function failed: {err!r} {out!r}"
Issue-Key: lambda.python-zoneinfo-resolves|_invoke_ok#3|AssertionError|assert err is None, f"the function failed: {err!r} {out!r}"
Issue-Key: lambda.python-ssl-default-context-verifies|_invoke_ok#3|AssertionError|assert err is None, f"the function failed: {err!r} {out!r}"
Issue-Key: lambda.python-threadpool-executor-runs|_invoke_ok#3|AssertionError|assert err is None, f"the function failed: {err!r} {out!r}"
Issue-Key: lambda.python-runtime-reports-configured-version|run#5|AssertionError|assert out["python_implementation"] == "CPython", (
Issue-Commit: f4372f081be041f1c06808452ec2531466fafcb7

# Lambda: the Python execution environment's extension modules, and the npm-bundle path — the five causes of the ten-cause runtime report that do NOT pass

> _RESOLVED in part, and split again: the COMPRESSION half of cause 3 (`bz2` and `lzma`) and the THREAD half of cause 4 (`concurrent.futures.ThreadPoolExecutor`, and the reentrant lock the standard library's conditions are built on) are fixed, and cause 5 (the implementation name) was fixed before them. The causes that remain host-reaching — the Node child process, the four npm-bundle checks whose own helper times out, `ctypes` loading the host's libc and the `ssl` library identity — are re-filed as `issues/things-that-do-not-work/20261111T081112Z-lambda-the-host-reaching-causes-of-the-managed-runtimes.md` so they are not recorded as working._

> _Split out of `issues/things-that-now-work/20261111T081112Z-lambda-managed-runtimes-are-stub-interpreters-10-node.md`, whose five Node.js causes are fixed (the core-module surface, the WHATWG globals, the password-based key derivation, the `exports` subpath map and the trailing separator on a reported path). This report carries the replay keys that do NOT pass, re-filed here so they are not recorded as working. Everything below is what that fix established about them; a later fixer starts from here rather than from scratch._

## Operation
Invoke

## Description
Five causes remain, each with its own failing checks and its own blocker.

Cause 1 of 5: a Node handler cannot start a child process.
AWS: recorded behavior only — `child_process.execSync("echo $((6*7))")` runs `/bin/sh -c` and
returns the shell's stdout, so the recorded Invoke answers `{"stdout": "42"}` with no
FunctionError.
Emulator: `require('child_process')` now loads (that half is fixed), and the CALL is refused with
node's own `spawnSync /bin/sh ENOSYS`, so the handler's catch block answers
`{"error": "Error", "message": "spawnSync /bin/sh ENOSYS"}`.
Why it is BLOCKED, and on what: starting a process needs a runtime this platform does not
provide ("Containers and external processes are not available to a generated emulator yet",
`vendor/engine/src/lib.rs`), and `services/service-com-amazonaws-lambda-awsgirapiservice/src/state/execution/node/core_modules/child_process.rs`
states that refusal deliberately rather than inventing a pid, a status or output. The same
capability is the whole subject of `issues/things-that-do-not-work/20261120T162942Z-lambda-the-nodejs22-x-managed-runtime-cannot-spawn.md`,
which carries this check's OTHER replay key (`run#4`): fixing it there fixes this key too.
Failing checks: lambda.nodejs-child-process-exec-sync

Cause 2 of 5: a Node package whose deployment bundle carries an npm dependency tree never
finishes its invoke.
AWS: recorded behavior only — a zip that bundles `node_modules` (pure-JS dependencies, an ESM
dependency with an exports map, or a native addon) is imported and the handler answers.
Emulator: the check never reaches a verdict, and the replay key of every one of the four is
`_npm_package#2|Timeout|r = subprocess.run(` — the timeout is in the CHECK's own
package-building helper (it runs npm in a subprocess), before any request reaches the emulator,
so no emulator-side change moves these four. What the emulator-side half of them exercises is
implemented and asserted on a bundle built in-process
(`services/service-com-amazonaws-lambda-awsgirapiservice/tests/all/node_runtime_module_surface.rs`,
`a_bare_specifier_resolves_through_the_packages_exports_map`): a `node_modules` tree in the
deployment package, a dependency's `exports` map, and the `import` condition of a dual-published
entry point all resolve. A native addon (`.node`) is a different capability: loading one needs a
host shared-library load, which the JS engine of this emulator does not have.
Why it is BLOCKED, and on what: the four keys cannot be reached from the emulator at all while
the check's own `subprocess.run` of npm times out. A fixer picking this up should first
establish whether the harness can build those packages in this environment.
Failing checks: lambda.nodejs-native-addon-from-npm-loads, lambda.nodejs-package-json-dependency-tree-imports, lambda.nodejs-package-json-esm-dependency-with-exports, lambda.nodejs-package-json-lodash-and-dayjs

Cause 3 of 5: the Python runtime is missing CPython extension modules.
AWS: recorded behavior only — a python3.x managed runtime is CPython with its standard library,
so `ctypes`, `bz2`/`lzma`, `sqlite3`, `subprocess` and `zoneinfo` all import, and `asyncio.run`
completes.
Emulator: the invoke answers 200 with X-Amz-Function-Error: Unhandled and errorType
`Runtime.ImportModuleError`, errorMessage `Unable to import module '<handler module>': No module
named '<ext>'` — observed `_ctypes`, `_bz2` and `_sqlite3`. `subprocess` now imports and its call
is refused (`OSError: [Errno 38] a Lambda function in this emulator starts no child process`),
and `asyncio` now imports and fails later, inside the standard library's own runner
(`AttributeError: loop_pid`, `runners.py` line 199).
Why it is BLOCKED, and on what: the native modules behind those names are the containment of
this runtime. `services/service-com-amazonaws-lambda-awsgirapiservice/src/state/execution/python.rs`
registers an ALLOW-LIST (`ALLOWED_NATIVE_MODULES`) filtered out of the engine's own pool, and the
engine is taken with `default-features = false` (see that crate's `Cargo.toml`), so `_ctypes`,
`_socket`, `_ssl`, `_posixsubprocess` and `_thread` are not even compiled into the build — which
is what makes "no host library load, no socket, no process, no host thread" true rather than
asserted. `ctypes` loading the host's libc and `subprocess` running a command are host
capabilities this platform does not provide; `_bz2`/`_lzma`/`_sqlite3` are not host-reaching
(they are compression and an in-process database) but are not in this build either, so reaching
them is a dependency-feature change with a C-toolchain question attached (`bzip2-sys`,
`libsqlite3-sys`) that was not taken as part of the Node fix. `asyncio.run` is a third thing
again: a gap in the frozen standard library's own event loop, reachable now that `_signal`
imports, and the next step for it is the `runners.py:199` frame above.
Failing checks: lambda.python-asyncio-run-completes, lambda.python-ctypes-loads-libc, lambda.python-lzma-and-bz2-round-trip, lambda.python-sqlite3-in-memory-query, lambda.python-subprocess-runs-a-command, lambda.python-zoneinfo-resolves

Cause 4 of 5: the Python runtime's `ssl` module is a stub, and a handler cannot start a thread.
AWS: recorded behavior only — `ssl.OPENSSL_VERSION` exists and `ssl.create_default_context()`
verifies a TLS peer; threads and `concurrent.futures.ThreadPoolExecutor` work inside a handler.
Emulator: `AttributeError: module 'ssl' has no attribute 'OPENSSL_VERSION'`, and
`RuntimeError: a Lambda function in this emulator cannot start a thread` from
`threading.Thread.start`.
Why it is BLOCKED, and on what: verifying a TLS peer means reaching one, which is egress this
emulator does not do (`src/state/execution/guest_tls.rs` and `guest_socket.rs` reach this
emulator's own planes and nothing else), and a host thread escapes every bound a run is held to
— the step latch, the deadline and the memory sample are all one thread's
(`src/state/execution/python.rs`'s bounds). Both refusals are deliberate. A faithful answer for
the thread half would be a cooperative `_thread` that runs the callable to completion on the
invocation's own thread; that was considered and not taken here, because it changes the
semantics of every threaded handler and belongs in its own change.
Failing checks: lambda.python-ssl-default-context-verifies, lambda.python-threadpool-executor-runs

Cause 5 of 5: the Python runtime does not report CPython as its implementation.
AWS: recorded behavior only — a function whose Runtime is `python3.12` reports
`sys.version_info[:2] == (3, 12)` and `platform.python_implementation() == "CPython"`.
Emulator: the VERSION half is fixed — `src/state/execution/python_release.rs` installs the
release the runtime identifier names, and the check now gets past
`assert out["version"] == [3, 12]` and fails on the implementation name instead.
Why it is BLOCKED, and on what: the interpreter an invocation runs in is RustPython, so
reporting `CPython` would be a claim about this build that is not true — the one thing the
execution environment must not do. A fixer taking this on should decide whether the
implementation name is a per-build identity (in which case this cause is not a defect and the
check cannot pass without fitting the recording) or whether the runtime should state the
implementation its identifier promises.
Failing checks: lambda.python-runtime-reports-configured-version

## Evidence
Cause 1: expected `{"stdout": "42"}`; got `{'error': 'Error', 'message': 'spawnSync /bin/sh ENOSYS'}`.
Cause 2: expected a served invoke; got 'Check timed out after 60.0s' for all four, at the check's own `_npm_package` helper (`subprocess.run`).
Cause 3: expected the module to import; got errorType 'Runtime.ImportModuleError' with "Unable to import module 'index': No module named '_ctypes'" / "… '_bz2'" / "… '_sqlite3'"; `subprocess` got `OSError: [Errno 38] a Lambda function in this emulator starts no child process`; `asyncio.run` got `AttributeError: loop_pid` at `runners.py` line 199.
Cause 4: expected ssl.OPENSSL_VERSION; got errorType 'AttributeError', errorMessage "module 'ssl' has no attribute 'OPENSSL_VERSION'". Expected ThreadPoolExecutor.map to run; got errorType 'RuntimeError', errorMessage 'a Lambda function in this emulator cannot start a thread'.
Cause 5: expected `python_implementation` 'CPython' for Runtime=python3.12; the version is now [3, 12] and the implementation name is this interpreter's own.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the behavioral suite.
2. Cause 1: CreateFunction with Runtime=nodejs22.x and a handler that calls
   `child_process.execSync`, then Invoke it.
3. Cause 2: run the four npm-bundle checks and observe where their 60 s budget is spent.
4. Causes 3-5: CreateFunction with Runtime=python3.12 and a handler that imports ctypes, bz2,
   lzma, sqlite3, subprocess or zoneinfo, runs `asyncio.run`, reads `ssl.OPENSSL_VERSION`,
   starts a ThreadPoolExecutor, or returns `platform.python_implementation()`, then Invoke it.
5. Re-replay exactly the check ids in trace_ids to confirm.
