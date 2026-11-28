Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda.nodejs-child-process-exec-sync lambda.nodejs-native-addon-from-npm-loads lambda.nodejs-package-json-dependency-tree-imports lambda.nodejs-package-json-esm-dependency-with-exports lambda.nodejs-package-json-lodash-and-dayjs lambda.python-ctypes-loads-libc lambda.python-ssl-default-context-verifies
Issue-Key: lambda.nodejs-child-process-exec-sync|_invoke_ok#3|AssertionError|assert err is None, f"the function failed: {err!r} {out!r}"
Issue-Key: lambda.nodejs-native-addon-from-npm-loads|_npm_package#2|Timeout|r = subprocess.run(
Issue-Key: lambda.nodejs-package-json-dependency-tree-imports|_npm_package#2|Timeout|r = subprocess.run(
Issue-Key: lambda.nodejs-package-json-esm-dependency-with-exports|_npm_package#2|Timeout|r = subprocess.run(
Issue-Key: lambda.nodejs-package-json-lodash-and-dayjs|_npm_package#2|Timeout|r = subprocess.run(
Issue-Key: lambda.python-ctypes-loads-libc|_invoke_ok#3|AssertionError|assert err is None, f"the function failed: {err!r} {out!r}"
Issue-Key: lambda.python-ssl-default-context-verifies|_invoke_ok#3|AssertionError|assert err is None, f"the function failed: {err!r} {out!r}"
Issue-Commit: f4372f081be041f1c06808452ec2531466fafcb7

# Lambda: the host-reaching causes of the managed runtimes — the three that do NOT pass

> _Split out of `issues/things-that-now-work/20261111T081112Z-lambda-the-python-execution-environment-and-the-npm-bundle-path.md`, whose compression and thread-pool causes are fixed (the `bz2`/`lzma` extension modules the standard library is written over, the reentrant lock's own surface and the `concurrent.futures` thread pool). This report carries the replay keys that do NOT pass, re-filed here so they are not recorded as working. Everything below is what that fix established about them; a later fixer starts from here rather than from scratch._

## Operation
Invoke

## Description
Three causes remain, each with its own failing checks and its own blocker.

Cause 1 of 3: a Node handler cannot start a child process.
AWS: recorded behavior only — `child_process.execSync("echo $((6*7))")` runs `/bin/sh -c` and
returns the shell's stdout, so the recorded Invoke answers `{"stdout": "42"}` with no
FunctionError.
Emulator: `require('child_process')` loads and the CALL is refused with node's own
`spawnSync /bin/sh ENOSYS`, so the handler's catch block answers
`{"error": "Error", "message": "spawnSync /bin/sh ENOSYS"}`.
Where it belongs: this key is the SECOND key of a check whose other key
(`run#4`) belongs to `issues/things-that-do-not-work/20261120T162942Z-lambda-the-nodejs22-x-managed-runtime-cannot-spawn.md`,
which is the whole subject of that capability — fixing it there fixes this key too. The PYTHON
half of the same capability is already served in-process
(`services/service-com-amazonaws-lambda-awsgirapiservice/src/state/execution/guest_child_process.rs`,
over `service_common::posix_shell_session`), so the Node side is the one shell session reached
from the other runtime's `child_process` rather than a new capability. Until then
`src/state/execution/node/core_modules/child_process.rs` states the refusal rather than
inventing a pid, a status or output.
Failing checks: lambda.nodejs-child-process-exec-sync

Cause 2 of 3: a Node package whose deployment bundle carries an npm dependency tree never
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

Cause 3 of 3: `ctypes` cannot load the host's libc, and the `ssl` module states no identity for
the TLS library behind it.
AWS: recorded behavior only — a python3.x managed runtime is CPython with its standard library,
so `ctypes.CDLL("libc.so.6")` loads the host's own C library and calls into it, and
`ssl.OPENSSL_VERSION` names the OpenSSL build that runtime links.
Emulator: the ctypes invoke answers 200 with X-Amz-Function-Error: Unhandled and errorType
`Runtime.ImportModuleError`, errorMessage `Unable to import module 'index': No module named
'_ctypes'`; the ssl invoke answers errorType `AttributeError`, errorMessage
`module 'ssl' has no attribute 'OPENSSL_VERSION'`.
Why it is BLOCKED, and on what: loading the host's libc is a host shared-library load, which is
the containment of this runtime — `src/state/execution/python.rs` registers an ALLOW-LIST
(`ALLOWED_NATIVE_MODULES`) and the engine is taken with `default-features = false`, so `_ctypes`
is not compiled into the build at all, and a `ctypes` that answered would be a claim about a
library nothing loaded. The `ssl` half is two questions at once: the NAMES are a gap this
runtime's own `src/state/execution/guest_tls.rs` doc claims to have ("the NAMES a program reads
off `ssl` … are all here"), but the value of `OPENSSL_VERSION` is the identity of a TLS build
this emulator does not link — its sessions are negotiated by the stack this binary really has —
and the rest of the check verifies a TLS PEER, which means reaching one. A fixer taking this on
has to decide the same question the implementation identity of the interpreter was decided by
(`src/state/execution/python_release.rs` restates the engine as the runtime it advertises)
before the name can carry a value, and then whether the peer the check verifies is one this
emulator itself hosts.
Failing checks: lambda.python-ctypes-loads-libc, lambda.python-ssl-default-context-verifies

## Evidence
Cause 1: expected `{"stdout": "42"}`; got `{'error': 'Error', 'message': 'spawnSync /bin/sh ENOSYS'}`.
Cause 2: expected a served invoke; got 'Check timed out after 60.0s' for all four, at the check's own `_npm_package` helper (`subprocess.run`).
Cause 3: expected the module to import; got errorType 'Runtime.ImportModuleError' with "Unable to import module 'index': No module named '_ctypes'". Expected ssl.OPENSSL_VERSION; got errorType 'AttributeError', errorMessage "module 'ssl' has no attribute 'OPENSSL_VERSION'".

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the behavioral suite.
2. Cause 1: CreateFunction with Runtime=nodejs22.x and a handler that calls
   `child_process.execSync`, then Invoke it.
3. Cause 2: run the four npm-bundle checks and observe where their 60 s budget is spent.
4. Cause 3: CreateFunction with Runtime=python3.12 and a handler that imports ctypes or reads
   `ssl.OPENSSL_VERSION`, then Invoke it.
5. Re-replay exactly the check ids in trace_ids to confirm.
