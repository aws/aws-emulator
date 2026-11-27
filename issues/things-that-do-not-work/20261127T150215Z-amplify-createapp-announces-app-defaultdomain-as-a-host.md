Issue-Source: trace-sweep-divergence
Issue-Trace: amplify-environment-variables-replace-semantics
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: amplify-environment-variables-replace-semantics amplify-job-start-and-manage
Issue-Shape-Id: com.amazonaws.amplify#Amplify
Issue-Key: amplify|CreateApp|2xx->2xx|$.app.defaultDomain
Issue-Commit: 680677ada8878ce293f02ee08799bbda12b1d232

# amplify: CreateApp announces App.defaultDomain as a host the emulator does not serve

> _Note: this issue was identified against the code base at commit `680677ada8878ce293f02ee08799bbda12b1d232`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApp

## Description
One net-new cause from the WIRE corpus: CreateApp answers 200 with the right shape, but the hosting address it publishes in `App$defaultDomain` is a host that is NOT one of the emulator's own hosts, so the one address the API hands a client for that app's hosting leads away from the emulator.

Related already-open issues, named for the fixer, none of which carries this fixture/step replay key: 20261111T130904Z-amplify-2-net-new-causes-app-defaultdomain-is--r2.md (same family of cause, but from the BEHAVIORAL corpus and describing the now-drifted value 'localhost:<port>'), 20261121T032742Z-amplify-the-branch-hosting-host-composed-from-app.md (behavioral, the branch host composed from the domain), and 20261109T143214Z-amplify-s3-every-s3-sourced-deployment-check-stops.md (the same family under the `amplify__s3` heading only). An open issue that looks similar does not cover this step, so it is filed here with its own replay key.

Cause 1 of 1: `CreateApp` reports `app.defaultDomain` as `<appId>.localhost` — a generated name in the reserved loopback zone that the emulator neither serves on nor announces as one of its own hosts — where AWS reports a hosting domain the client that was handed it can resolve and dial.
  AWS: Smithy (services/service-com-amazonaws-amplify-amplify/smithy-model/com-amazonaws-amplify-amplify.json) models CreateApp -> POST /apps, 200, output `app` @required -> App; `App$defaultDomain` targets `DefaultDomain` (string, @length 1..1000) and is documented "The default domain for the Amplify app." The model fixes no pattern, so the FORM and the REACHABILITY are recorded behavior only: AWS returns a bare dotted DNS hostname (no scheme, no port, no path) whose first label is the app's own `appId` and which resolves to Amplify's hosting edge, so `https://<Branch$displayName>.<App$defaultDomain>/` — the composition Amplify documents, since `Branch$displayName` "is used as the default domain prefix" — is answered by the service. The requirement is that the announced domain be a host the emulator itself serves on and announces as its own; the concrete zone, host labels and port are per-run environment values and are never the requirement.
  Emulator: the replayed CreateApp returns 200 with `$.app.defaultDomain` = `<appId>.localhost` (recorded in this run as 'd1kwt6g9iaogsf.localhost'; the app-id label is a per-run generated value, the defect is the host). The replay verdict is `host_not_own`: the announced host is not one of the emulator's own hosts. The value is composed by services/service-com-amazonaws-amplify-amplify/src/state/hosting_address.rs:65-75 (`announced_default_domain` -> `format!("{app_id}.{zone}")` over `announced_zone_of_authority`), and the only address the hosting edge is really reachable at is the private path form `/_aws/amplify/<appId>/<displayName>` (hosting_address.rs:150-152) — an address no AWS response ever names. hosting_address.rs:103-164 only compares an INBOUND Host against `hosted_branch_host`; nothing registers `<appId>.localhost`, or any host built from it, on a listener of this emulator.
  Why: `defaultDomain` is the only hosting address the Amplify API gives a client, and a client uses it two ways — it prefixes a branch label to it to reach the live site, and it puts it in DNS/CNAME and certificate configuration for a custom domain. Against the emulator both uses fail: a GET of `https://<displayName>.<defaultDomain>/` never reaches the emulator, so a deployment the control plane reports SUCCEED is unverifiable and the whole branch hosting data plane (published bytes, 404 for an unpublished path, redirect rules, custom headers, branch basic auth) is unobservable, while a client that stores the value as a resolvable hosting domain stores one that resolves nowhere near this process.
  Fix: the announced-domain composition and the hosting edge's addressing in this crate — src/state/hosting_address.rs (`announced_default_domain`, `hosted_branch_host`, `hosted_site_host`) together with the edge that claims hosting requests in src/service.rs — must announce in `defaultDomain` the host of a listener this emulator actually serves on, and register that host (and `<displayName>.<defaultDomain>` for every branch) as one of the emulator's own hosts so the address CreateApp publishes routes back to the emulator that published it.
  Traces: amplify-environment-variables-replace-semantics#task#1 (representative) and amplify-job-start-and-manage#setup#1, the two recorded CreateApp steps in this cluster.

## Evidence
[mismatch] fixture amplify-environment-variables-replace-semantics, step task#1 — CreateApp (service amplify)
  Expected status 200, actual status 200 — the status and the response shape matched; the divergence is the published value.
  Divergence detail: `$.app.defaultDomain: host_not_own (expected '<shaped-not-asserted>', got 'd1kwt6g9iaogsf.localhost')` — the recorded fixture does not assert the literal AWS value (AWS records `<appId>.amplifyapp.com`), it asserts that the published host is one the emulator serves; the emulator's value fails that.
  Cluster key: `service=amplify|CreateApp|status=200->200|keys=$.app.defaultDomain`; affected_count 2 — also amplify-job-start-and-manage#setup#1.
  The app-id label `d1kwt6g9iaogsf` is a per-run generated id and is not part of the requirement; the requirement is that the announced host be served/announced by the emulator.
  Expected: `app.defaultDomain` is a bare hostname (no scheme, no port, no path) whose first label is the `appId` CreateApp reports in the same response, AND which is one of the emulator's own hosts, so that `https://<Branch$displayName>.<app.defaultDomain>/<path>` is answered by this emulator.
  Source: services/service-com-amazonaws-amplify-amplify/src/state/hosting_address.rs:65-75 (`announced_default_domain`), :103-164 (`hosted_branch_host`/`hosted_site_host` compare an inbound Host only; no listener claims that host), :150-152 (the private path form that IS reachable).

## Steps to Reproduce
1. Replay fixture `amplify-environment-variables-replace-semantics`, step `task#1` (equivalently `amplify-job-start-and-manage`, step `setup#1`).
2. Or by hand: r = amplify.create_app(name=<unique>); read r['app']['appId'] and r['app']['defaultDomain'].
   Observed: HTTP 200 with defaultDomain '<appId>.localhost' — a generated name in the reserved loopback zone that the emulator does not serve on and does not announce as one of its own hosts (replay verdict host_not_own).
   Expected: defaultDomain is a bare, port-free hostname whose first label is the returned appId and whose host is one of the emulator's own hosts.
3. Compose the documented hosting address: amplify.create_branch(appId=app_id, branchName='main'); display = amplify.get_branch(appId=app_id, branchName='main')['branch']['displayName']; GET https://{display}.{defaultDomain}/ .
   Observed: the host is not one of the emulator's own hosts, so the request never reaches the emulator.
   Expected: the emulator answers that address (200 with the published bytes after a SUCCEED deployment; 404 for a path the deployment does not contain).
