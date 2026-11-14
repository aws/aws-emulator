Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: eks.hpa-min-replicas-above-max-replicas-rejected eks.hpa-scales-out-under-cpu-load-and-back-in-when-it-stops eks.hpa-target-without-a-cpu-request-is-blocked-from-scaling eks.eks-pod-identity-credentials-are-scoped-and-an-unallowed-bucket-is-denied eks.e2e-certificate-authority-activation-to-teardown eks.e2e-certificate-authority-issue-activate-disable-delete
Issue-Shape-Id: com.amazonaws.eks#AWSWesleyFrontend
Issue-Key: eks.hpa-min-replicas-above-max-replicas-rejected|_sh#3|AssertionError|raise AssertionError(
Issue-Key: eks.hpa-scales-out-under-cpu-load-and-back-in-when-it-stops|_sh#3|AssertionError|raise AssertionError(
Issue-Key: eks.hpa-target-without-a-cpu-request-is-blocked-from-scaling|_sh#3|AssertionError|raise AssertionError(
Issue-Key: eks.eks-pod-identity-credentials-are-scoped-and-an-unallowed-bucket-is-denied|_routed_request#20|PublishedUrlRejected|target, route = ctx.endpoint_route(url)
Issue-Key: eks.e2e-certificate-authority-activation-to-teardown|_poll_until#24|Timeout|time.sleep(interval_s)
Issue-Key: eks.e2e-certificate-authority-issue-activate-disable-delete|_wait_ca_gone#21|Timeout|time.sleep(interval_s)
Issue-Commit: bbe224ee8535eddb09d8153d70c72e25d751373e

# eks: 2 net-new root causes — cluster.endpoint is published on a real eks.amazonaws.com host so the Kubernetes data plane is unreachable, and a successor certificate authority never finishes distribution so CA rotation never completes

> _Note: this issue was identified against the code base at commit `bbe224ee8535eddb09d8153d70c72e25d751373e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeCluster

## Description
Two distinct net-new root causes in the eks service. Both must be closed before submitting.

Cause 1 of 2: DescribeCluster/CreateCluster publish cluster.endpoint as a real-AWS EKS host that the emulator does not serve, so every Kubernetes API (data-plane) interaction against an emulated cluster is unreachable.
Failing checks: eks.hpa-min-replicas-above-max-replicas-rejected, eks.hpa-scales-out-under-cpu-load-and-back-in-when-it-stops, eks.hpa-target-without-a-cpu-request-is-blocked-from-scaling, eks.eks-pod-identity-credentials-are-scoped-and-an-unallowed-bucket-is-denied.
  AWS: Smithy (services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json): Cluster.endpoint targets String and is documented as "The endpoint for your Kubernetes API server"; Cluster.certificateAuthority targets Certificate (the cluster's certificate-authority-data). On AWS that endpoint is an address that actually serves the cluster's Kubernetes API: a client builds a kubeconfig from endpoint + certificateAuthority.data and `kubectl` (or a plain HTTPS request to <endpoint>/api/v1/namespaces) reaches a live API server. The endpoint is a server address, not a literal — the requirement is that the host published is one the service answering the request actually serves.
  Emulator: the endpoint handed back is a real-AWS hostname of the form https://<32-hex-id>.<3-char>.<region>.eks.amazonaws.com, which is not an address of the emulator. Two symptoms, one cause: (a) the three HPA checks run `kubectl get nodes -o json` with a kubeconfig built from the published endpoint and get rc=1 with `Unable to connect to the server: dial tcp: lookup <id>.<...>.<region>.eks.amazonaws.com on 10.0.0.2:53: no such host` (surfaced by the check as `AssertionError: DATAPLANE_UNIMPLEMENTED: command failed (rc=1): kubectl get nodes -o json`); (b) the pod-identity check routes a request to <endpoint>/api/v1/namespaces and the harness refuses the published address: `simparity.published.PublishedUrlRejected: endpoint_url was given https://<id>.<...>.<region>.eks.amazonaws.com/api/v1/namespaces (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener`. The emulator announced an address other than the one it is reached on, so this is the emulator's refusal, not a runner network problem.
  Why: a client that does the documented thing — read cluster.endpoint, build a kubeconfig, talk to the Kubernetes API — resolves a host the emulator does not serve and either fails DNS or escapes to real AWS. Every in-cluster behavior (HorizontalPodAutoscaler admission and scaling, pod identity credential scoping, namespace/pod/service operations) is unreachable, so an emulated EKS cluster can be created and described but never used.
  Fix: the endpoint publication in the EKS service crate — the CreateCluster/DescribeCluster handlers and the cluster record in the service's state that mint cluster.endpoint must publish one of the emulator's own hosts/ports (the per-resource listener that actually serves the cluster's Kubernetes API), together with a certificateAuthority.data that validates that listener.
  Traces: eks.hpa-min-replicas-above-max-replicas-rejected, eks.hpa-scales-out-under-cpu-load-and-back-in-when-it-stops, eks.hpa-target-without-a-cpu-request-is-blocked-from-scaling, eks.eks-pod-identity-credentials-are-scoped-and-an-unallowed-bucket-is-denied.
  Relation to already-open issues: 20261110T203834Z-eks-6-net-new-root-causes-cluster-endpoint.md and 20261109T160052Z-eks-3-net-new-root-causes-cluster-endpoint.md describe this same family for other checks, but neither carries the replay keys of these four checks, so the suite reports them unowned and they are filed here.

Cause 2 of 2: a certificate authority appended with CreateCertificateAuthority never reaches distributionStatus COMPLETE and its Update never reaches a terminal status, so a CA rotation can never be completed and nothing downstream of a rotation is observable.
Failing checks: eks.e2e-certificate-authority-activation-to-teardown, eks.e2e-certificate-authority-issue-activate-disable-delete.
  AWS: Smithy (same model): CreateCertificateAuthority (POST /clusters/{clusterName}/certificate-authorities) returns {update: Update, certificateAuthority: CertificateAuthoritySummary}; CertificateAuthorityDistributionStatus = IN_PROGRESS | COMPLETE | FAILED | DELETING and the member documentation states a successor CA can only be activated once its distribution status is COMPLETE; CertificateAuthoritySigningStatus = NOT_USED | ACTIVATING | IN_USE. On AWS the lifecycle settles: the appended successor starts NOT_USED/IN_PROGRESS and progresses to COMPLETE; ActivateCertificateAuthority then returns an Update whose DescribeUpdate status reaches Successful, after which the successor reads IN_USE and the outgoing CA reads NOT_USED and becomes deletable. Only then are the terminal refusals observable: DeleteCertificateAuthority on the sole IN_USE signer answers 409 ResourceInUseException with the fixed sentence "Cannot delete the active certificate authority. Activate a different CA first." (whole sentence is fixed text, no variable part); DescribeCertificateAuthority of a deleted id answers 404 ResourceNotFoundException with EKS's wrapped not-found text whose fixed head is "certificate authority not found (Service: EksCertificateAuthority, Status Code: 404, Request ID: " and fixed tail ") (SDK Attempt Count: 1)" (the request id between them is a per-run value, not a requirement); ActivateCertificateAuthority of a deleted id answers 404 ResourceNotFoundException with "Certificate authority <certificateAuthorityId> is not in the trusted list for cluster <clusterName>" — fixed text with the CA id and cluster name taken from the request.
  Emulator: neither check reaches any assertion; both are recorded only as "Error: Check timed out after 60.0s". Each creates an ACTIVE cluster, asserts the initial sole CA is IN_USE and that CreateCertificateAuthority returns a successor with signingStatus NOT_USED (those calls pass), and then sleeps in a poll of DescribeCertificateAuthority for distributionStatus in (COMPLETE, FAILED) at 15 s intervals until the budget is gone. A settled lifecycle would let these checks finish in seconds — the time spent in the poll is the symptom of a status that never advances. Hypothesis: the CA record is created with a distributionStatus that is never advanced and the associated CertificateAuthorityUpdate is never advanced to Successful, so the successor is never activatable.
  Why: CA rotation is exactly create-successor → wait for COMPLETE → activate → retire the old CA. A client doing that against the emulator blocks forever on the documented precondition, and because no rotation can complete, the post-rotation behaviors (deleting the active signer, deleting a retired NOT_USED CA, reading or activating a deleted CA id) can never be exercised through the public API.
  Fix: the certificate-authority lifecycle/state machinery in the eks service crate — the CreateCertificateAuthority and ActivateCertificateAuthority handlers and the CA/update records in the service's state must advance distributionStatus IN_PROGRESS → COMPLETE and advance the associated Update to a terminal Successful, flipping the successor's signingStatus to IN_USE and the outgoing CA's to NOT_USED.
  Traces: eks.e2e-certificate-authority-activation-to-teardown, eks.e2e-certificate-authority-issue-activate-disable-delete.
  Relation to already-open issues: 20261106T022127Z-eks-a-successor-certificate-authority-never-finishes-distribution.md describes this same root cause from check eks.certificate-authority-delete-while-active-blocked, but does not carry these two checks' replay keys, so the suite reports them unowned and they are filed here.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the emulator built from this checkout (total 223/352; eks 208/311).

Cause 1 — published cluster endpoint is not an emulator host.
From eks.hpa-min-replicas-above-max-replicas-rejected (identical in eks.hpa-scales-out-under-cpu-load-and-back-in-when-it-stops and eks.hpa-target-without-a-cpu-request-is-blocked-from-scaling):
  AssertionError: DATAPLANE_UNIMPLEMENTED: command failed (rc=1): kubectl get nodes -o json
  stderr: Unable to connect to the server: dial tcp: lookup <32-hex>.<3-char>.<region>.eks.amazonaws.com on 10.0.0.2:53: no such host
From eks.eks-pod-identity-credentials-are-scoped-and-an-unallowed-bucket-is-denied:
  simparity.published.PublishedUrlRejected: endpoint_url was given https://<32-hex>.<3-char>.<region>.eks.amazonaws.com/api/v1/namespaces (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener
Expected (AWS): DescribeCluster(name).cluster.endpoint is an address that serves the cluster's Kubernetes API, so GET <endpoint>/api/v1/namespaces answers a Kubernetes API response and `kubectl get nodes -o json` exits 0.
Actual (emulator): the endpoint's host is a real-AWS eks.amazonaws.com name the emulator does not serve; no Kubernetes API request ever reaches the emulator.

Cause 2 — CA distribution never completes.
eks.e2e-certificate-authority-activation-to-teardown and eks.e2e-certificate-authority-issue-activate-disable-delete both record exactly:
  Error: Check timed out after 60.0s
and no assertion failure, i.e. neither got past its lifecycle poll:
  _poll_bounded(lambda: eks.describe_certificate_authority(clusterName=<cluster>, certificateAuthorityId=<successor>)["certificateAuthority"], lambda ca: ca["distributionStatus"] in ("COMPLETE", "FAILED"), timeout_s=480.0, interval_s=15.0)
The calls made before the poll do pass: ListCertificateAuthorities returns exactly one CA with signingStatus IN_USE, and CreateCertificateAuthority returns a successor with signingStatus NOT_USED plus an update id.
Expected (AWS): the successor's distributionStatus reaches COMPLETE; ActivateCertificateAuthority's update reaches Successful; the successor then reads IN_USE and the original settles NOT_USED and deletes; DescribeCertificateAuthority on the deleted id answers 404 ResourceNotFoundException with the wrapped "certificate authority not found (Service: EksCertificateAuthority, Status Code: 404, Request ID: …) (SDK Attempt Count: 1)" text, and ActivateCertificateAuthority on it answers 404 ResourceNotFoundException "Certificate authority <id> is not in the trusted list for cluster <cluster>"; while rotation is incomplete, DeleteCertificateAuthority of the sole signer answers 409 ResourceInUseException "Cannot delete the active certificate authority. Activate a different CA first."
Actual (emulator): distributionStatus never becomes COMPLETE or FAILED, so none of the above is ever evaluated.

Smithy consulted: Cluster.endpoint (String, "The endpoint for your Kubernetes API server"), Cluster.certificateAuthority (Certificate); CertificateAuthorityDistributionStatus = [IN_PROGRESS, COMPLETE, FAILED, DELETING]; CertificateAuthoritySigningStatus = [NOT_USED, ACTIVATING, IN_USE]; DeleteCertificateAuthority errors include ResourceInUseException (409); DescribeCertificateAuthority/ActivateCertificateAuthority errors include ResourceNotFoundException (404).

## Steps to Reproduce
1. git clone the project repository; build and boot the emulator from this checkout.
2. Replay the behavioral suite (simparity-behavioral-aws-v1).

Cause 1 (cluster endpoint):
3. Re-run eks.hpa-min-replicas-above-max-replicas-rejected, eks.hpa-scales-out-under-cpu-load-and-back-in-when-it-stops, eks.hpa-target-without-a-cpu-request-is-blocked-from-scaling and eks.eks-pod-identity-credentials-are-scoped-and-an-unallowed-bucket-is-denied.
4. Manually: CreateCluster with two AZ-distinct subnets, wait for status ACTIVE, then DescribeCluster(name) and read cluster.endpoint. Expect its host to be one of the emulator's own hosts/ports. Observe a real-AWS <id>.<...>.<region>.eks.amazonaws.com host instead.
5. Build a kubeconfig from cluster.endpoint plus cluster.certificateAuthority.data and run `kubectl get nodes -o json`; expect rc=0 and a node list. Observe "Unable to connect to the server: dial tcp: lookup …eks.amazonaws.com…: no such host". Likewise GET <endpoint>/api/v1/namespaces never reaches the emulator.

Cause 2 (certificate-authority distribution):
6. Re-run eks.e2e-certificate-authority-activation-to-teardown and eks.e2e-certificate-authority-issue-activate-disable-delete.
7. Manually: CreateCluster, wait ACTIVE; ListCertificateAuthorities(clusterName) -> one CA, signingStatus IN_USE.
8. CreateCertificateAuthority(clusterName) -> note certificateAuthority.id (signingStatus NOT_USED) and update.id.
9. Poll DescribeCertificateAuthority(clusterName, successor id): expect distributionStatus to reach COMPLETE, and DescribeUpdate(name=cluster, updateId=update.id) to reach Successful. Observe neither ever settles.
10. Once COMPLETE (unreachable today): ActivateCertificateAuthority(clusterName, successor id), poll its update to Successful, expect successor IN_USE and the original NOT_USED; DeleteCertificateAuthority(original) succeeds and the id leaves ListCertificateAuthorities; DescribeCertificateAuthority(original) -> 404 ResourceNotFoundException with the wrapped not-found text; ActivateCertificateAuthority(original) -> 404 ResourceNotFoundException "Certificate authority <id> is not in the trusted list for cluster <cluster>".
