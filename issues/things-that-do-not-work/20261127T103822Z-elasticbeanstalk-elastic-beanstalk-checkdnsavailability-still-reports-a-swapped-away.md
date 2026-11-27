Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elasticbeanstalk.e2e-swap-cname-then-verify-dns-and-traffic-consequence
Issue-Shape-Id: com.amazonaws.elasticbeanstalk#AWSElasticBeanstalkService
Issue-Key: elasticbeanstalk.e2e-swap-cname-then-verify-dns-and-traffic-consequence|run#24|AssertionError|assert dns_a["Available"] is True, (
Issue-Commit: 96db52f5b4a4b596ab227bd8becc0930845733a7

# Elastic Beanstalk CheckDNSAvailability still reports a swapped-away CNAME prefix as unavailable (1 cause)

> _Note: this issue was identified against the code base at commit `96db52f5b4a4b596ab227bd8becc0930845733a7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CheckDNSAvailability

## Description
One net-new root cause in the Elastic Beanstalk control plane. The replay did NOT report this failing check as owned by, or already filed under, any open issue. The open issue 20261123T012852Z-elasticbeanstalk-elastic-beanstalk-swapenvironmentcnames-moves-no-traffic-the-served.md describes a neighbouring SwapEnvironmentCNAMEs defect (the data plane does not follow the exchanged CNAME) and 20261124T063442Z-elasticbeanstalk-elastic-beanstalk-environment-cname-is-published-as-host.md describes the CNAME's representation; they are named for context only, not as cover — neither carries this failure's replay key, and this cause is in CheckDNSAvailability's reservation bookkeeping, not in the data plane.

Cause 1 of 1: after SwapEnvironmentCNAMEs exchanges two environments' CNAMEs, CheckDNSAvailability still answers Available=false for each environment's ORIGINAL CNAME prefix. The emulator keeps a prefix reserved against the prefix string itself (or re-derives the reservation from the live CNAME values) instead of releasing the reservation that the swap vacated.

AWS: Smithy: CheckDNSAvailability, input CheckDNSAvailabilityMessage {CNAMEPrefix: DNSCnamePrefix @required @length 4-63}, output CheckDNSAvailabilityResultMessage {Available: boolean, FullyQualifiedCNAME: DNSCname @length 1-255}, no modeled errors; SwapEnvironmentCNAMEs, input {SourceEnvironmentId/SourceEnvironmentName @length 4-40, DestinationEnvironmentId/DestinationEnvironmentName @length 4-40}, output Unit, no modeled errors. The post-swap availability answer is recorded behavior only: the swap releases each prefix's CNAME reservation without transferring it to the other environment, so after the exchange CheckDNSAvailability on BOTH original prefixes answers HTTP 200 with Available=true, even though each CNAME now serves the other environment. The prefix strings and environment names are values taken from the request and the emulator's state; the requirement is the release rule (a prefix that no environment holds a reservation on answers Available=true), not any one run's prefix.
Emulator: the call succeeds (HTTP 200) but answers Available=false for the original prefix of the source environment, e.g. {'Available': False} for the prefix taken from the source environment's pre-swap CNAME. The exchange itself is observed (DescribeEnvironments reports each environment holding the other's former CNAME and both EnvironmentIds unchanged), so the swap updated the CNAME values but left the old prefix reservations standing; the check fails its first post-swap availability assertion with "prefix_a '<prefix>' still reports held after the swap released its reservation: {'Available': False, ...}".
Why: a client that provisions a replacement environment after a blue/green cut-over probes CheckDNSAvailability to pick a free prefix. It is told the prefix it just vacated is still taken, so automation that reclaims or recycles a prefix after a swap loops or picks a different name, and a client cannot distinguish this from a genuine reservation held by another account. Conversely, nothing in the emulator ever reports a vacated prefix as free, so the AWS quirk this behaviour is built on (swap releases, does not transfer, the reservation) is unobservable.
Fix: the CheckDNSAvailability handler in the elasticbeanstalk service together with the SwapEnvironmentCNAMEs handler and the CNAME-prefix reservation state they share — the swap must drop both environments' prefix reservations (not re-point them), and CheckDNSAvailability must answer Available=true (with the FullyQualifiedCNAME for that prefix) for any prefix no reservation is held on.
Traces: elasticbeanstalk.e2e-swap-cname-then-verify-dns-and-traffic-consequence.

## Evidence
Cause 1: [mismatch] elasticbeanstalk.e2e-swap-cname-then-verify-dns-and-traffic-consequence — AssertionError at check line 68: "prefix_a 'sim2real-swp-a-<suffix>' still reports held after the swap released its reservation: {'Available': False, 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}" (the prefix suffix and request id are per-run values).

Expected (AWS), after swap_environment_cnames(SourceEnvironmentName=<env-a>, DestinationEnvironmentName=<env-b>):
  - DescribeEnvironments: env-a holds <cname-b>, env-b holds <cname-a>, both EnvironmentIds unchanged (this part the emulator already satisfies).
  - CheckDNSAvailability(CNAMEPrefix=<prefix of cname-a>) -> 200, Available=true.
  - CheckDNSAvailability(CNAMEPrefix=<prefix of cname-b>) -> 200, Available=true.
  The fixed requirement is Available=true for a prefix whose reservation the swap released; <prefix of cname-a>/<prefix of cname-b> are values derived from the environments the request named.

Actual (emulator): CheckDNSAvailability(CNAMEPrefix=<prefix of cname-a>) -> 200 with Available=false (the second probe is never reached because the first assertion fails). No error code or message is involved: the status is correct and the member value is wrong.

## Steps to Reproduce
1. CreateApplication(ApplicationName=<app>).
2. ListAvailableSolutionStacks and pick a Node.js solution stack.
3. CreateEnvironment twice under <app> — env-a and env-b — with EnvironmentType=SingleInstance (plus an instance profile/service role when IAM is reachable). Poll DescribeEnvironments until both Status == 'Ready' and record each CNAME and EnvironmentId.
4. SwapEnvironmentCNAMEs(SourceEnvironmentName=env-a, DestinationEnvironmentName=env-b); poll DescribeEnvironments until env-a reports env-b's former CNAME and env-b reports env-a's former CNAME (the EnvironmentIds must not change).
5. CheckDNSAvailability(CNAMEPrefix=<the first label of env-a's pre-swap CNAME>) — the emulator answers Available=false; AWS answers Available=true because the swap released that prefix's reservation.
6. Same for <the first label of env-b's pre-swap CNAME>.
