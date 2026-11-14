Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ds__ec2.e2e-managed-ad-with-ldaps-and-resets-lifecycle ds.ldaps-enable-disable-server-enforced
Issue-Key: ds__ec2.e2e-managed-ad-with-ldaps-and-resets-lifecycle|phase#6|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Key: ds.ldaps-enable-disable-server-enforced|_wait_ldaps_status#15|Timeout|time.sleep(interval_s)
Issue-Commit: 52a79640a1ceceb858c6fc834fcbcb6a886924f9

# ds: 2 net-new behavioral root causes — an Active Managed Microsoft AD still serves an EMPTY DnsIpAddrs (the e2e LDAPS deploy lifecycle dies in its solve phase), and the EnableLDAPS → Enabled → DisableLDAPS → Disabled trajectory never converges inside the replay budget

> _Note: this issue was identified against the code base at commit `52a79640a1ceceb858c6fc834fcbcb6a886924f9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateMicrosoftAD, DescribeDirectories, EnableLDAPS

## Description
Two independent net-new root causes in the Directory Service (ds) emulator, found by suite simparity-behavioral-ga-prod-v1. They are independent and BOTH must be closed before this issue is submitted. Neither of the two failing checks below was marked ALREADY FILED or OWNED by the replay report, so neither replay key is carried by any open issue.

=== Cause 1 of 2: DescribeDirectories reports an Active Managed Microsoft AD with an EMPTY DnsIpAddrs, so the whole deploy→solve→cleanup→destroy LDAPS lifecycle dies at its first post-create read ===

AWS: Smithy (services/service-com-amazonaws-directoryservice-directoryservice-20150416/smithy-model/com-amazonaws-directoryservice-directoryservice-20150416.json) — DescribeDirectories returns DirectoryDescriptions; DirectoryDescription carries DnsIpAddrs (a list of IpAddr) and VpcSettings (DirectoryVpcSettingsDescription with VpcId/SubnetIds/AvailabilityZones/SecurityGroupId). The model describes the envelope only, so the placement rule is recorded behavior only: once the domain controllers are placed (i.e. by the time Stage is Active), a Managed Microsoft AD created with VpcSettings naming two subnets reports DnsIpAddrs holding exactly TWO addresses, one CONTAINED IN each of the two subnet CIDRs the caller passed. The addresses are derived by the service from those subnets; they are not caller-supplied and are not free-form. The list is legitimately empty only while the directory is still being built.

Emulator: for a directory the emulator itself reports as Stage 'Active', DescribeDirectories answers DnsIpAddrs = [] (absent/empty), permanently. The e2e check re-reads the directory independently of its own polls, asserts Stage == 'Active' (that part MATCHES), and then fails: "an Active directory serves DNS addresses [] that should sit in [IPv4Network('10.168.0.0/24'), IPv4Network('10.168.1.0/24')]". The two CIDRs in that message are this run's stack subnets and are NOT part of the requirement — the requirement is two addresses, one inside each subnet CIDR the CreateMicrosoftAD request named, whatever those CIDRs are.

Why: DnsIpAddrs is the only way a client learns where to point its resolvers or its conditional forwarders at a new managed directory. A client that reads them to configure DHCP option sets, a VPC resolver rule, a domain join, or an LDAP/LDAPS endpoint gets nothing from a directory the service simultaneously declares ready, and an automation that (correctly) waits for them to appear never proceeds. Here it also makes every LATER graded step of the lifecycle unreachable: the solve phase aborts before LDAPS enablement, the ListCertificates read, and the cleanup/destroy phases (DisableLDAPS→Disabled, DeregisterCertificate, DeleteDirectory releasing the ENIs and the controllers security group) are never exercised at all.

Fix: the controller-placement / address-derivation step of the directory state machine that feeds DescribeDirectories — services/service-com-amazonaws-directoryservice-directoryservice-20150416/src/state.rs (the Directory record and Directory::to_description, which is the single place a stored directory is rendered as DirectoryDescription) together with the create path that places the controllers in services/.../src/ops/create_microsoft_ad.rs. Allocate one address out of each requested subnet's CIDR as the controllers are placed and report them in DnsIpAddrs by the time the directory is reported Active.

Related (does NOT cover this): the open issue 20261024T081028Z-ds-directory-service-a-microsoft-ad-never-provisions-its.md records the same DnsIpAddrs defect as its cause 2, but it is grounded in — and carries the replay keys of — only ds__ec2.microsoft-ad-dns-ips-one-per-chosen-subnet-inside-its-cidr and ds__ec2.microsoft-ad-creates-a-controllers-security-group-and-one-eni-per-subnet-in-the-vpc, both of which fail as bare 60 s timeouts. This cause is grounded in a DIFFERENT check with a HARD assertion frame (the e2e lifecycle), which the report did not mark OWNED; it is named here so a fixer lands one change and re-replays both key sets rather than fixing it twice.

Traces: ds__ec2.e2e-managed-ad-with-ldaps-and-resets-lifecycle (AssertionError at check_ds__ec2_e2e_managed_ad_with_ldaps_and_resets_lifecycle.py:266, phase 2/4 'solve').

=== Cause 2 of 2: the EnableLDAPS → Enabled → DisableLDAPS → Disabled trajectory is never observed — the check makes no progress and is killed at the 60 s replay cap ===

AWS: Smithy — EnableLDAPS{DirectoryId (required, @pattern ^d-[0-9a-f]{10}$), Type (LDAPSType, required, only value 'Client')} returns an empty 200 body and models NoAvailableCertificateException / InvalidLDAPSStatusException / DirectoryUnavailableException / DirectoryDoesNotExistException / ClientException / InvalidParameterException / UnsupportedOperationException (all @httpError 400) and ServiceException (500); DisableLDAPS has the same input shape and the same error set minus NoAvailableCertificateException; DescribeLDAPSSettings returns LDAPSSettingsInfo (LDAPSSettingInfo{LDAPSStatus, LDAPSStatusReason, LastUpdatedDateTime}) and LDAPSStatus is the enum [Enabling, Enabled, EnableFailed, Disabled]. The end-to-end contract is recorded behavior only: with a self-signed ClientLDAPS certificate registered and in state Registered, EnableLDAPS(Type='Client') drives DescribeLDAPSSettings(Type='Client') to LDAPSStatus 'Enabled' (not 'EnableFailed' — the standalone-CA caveat in the admin guide does not block it), and a subsequent DisableLDAPS(Type='Client') drives it to 'Disabled'.

Emulator: the check never reaches either terminal reading. The harness kills it with "Error: Check timed out after 60.0s" and emits NO exception frame — the signature of a client that is still waiting on a state transition it has not been served. Nothing in the trajectory is refused: the emulator answers CreateMicrosoftAD, RegisterCertificate, DescribeCertificate, EnableLDAPS and DescribeLDAPSSettings without an error (EnableLDAPS is no longer the 501 scaffold it was when 20261025T030730Z-ds-enableldaps-stub.md was filed), so this is a convergence failure and not a refusal.

Hypothesis (neither the model nor the recording states this; it is the reading of the checkout a fixer should confirm first): the three asynchronous gates the check waits on are each still TRANSIENT on the client's first read and each then costs the client one full poll interval — directory Stage 'Requested' (30 s interval), certificate State 'Registering' (10 s interval), LDAPSStatus 'Enabling' (20 s interval) — which sums to exactly the 60.0 s cap the harness reports. In the checkout every one of those three transitions is owed to crate::state::TRANSITION_SPAN (= 500 ms) and materialised by ComAmazonawsDirectoryserviceDirectoryservice20150416State::advance_clock, so each gate is observable as transient to any caller that reads back promptly. The fixer must determine which gate the client is parked on before changing anything.

Why: a client cannot establish LDAPS at all against this emulator by the documented pattern (register a certificate, wait for Registered, EnableLDAPS, wait for Enabled, later DisableLDAPS, wait for Disabled) — it is left polling. Both the positive path (LDAPS actually reaching 'Enabled' on a freshly registered certificate) and the restore path ('Disabled' after DisableLDAPS) are therefore unverifiable, which is precisely the pair of state readings this check exists to grade.

Fix: the LDAPS status transition the enable owes and the gates ahead of it — services/service-com-amazonaws-directoryservice-directoryservice-20150416/src/ops/enable_ldaps.rs (which records LDAPS_STATUS_ENABLING with status_due_at = now + TRANSITION_SPAN), src/ops/register_certificate.rs (CERTIFICATE_STATE_REGISTERING with state_due_at = now + TRANSITION_SPAN), and the directory stage/transition machinery plus advance_clock in src/state.rs that settles all three. Any change here MUST be reconciled with the two open issues that constrain the SAME machinery in the opposite direction — 20261024T015148Z-ds-directory-service-a-new-microsoft-ad-is-instantly.md cause 1 requires a new directory to be observable in stage 'Creating' rather than jumping to 'Active', and 20261027T012551Z-ds-4-net-new-behavioral-root-causes-createcomputer.md cause 2 requires EnableLDAPS's NoAvailableCertificateException gate and its convergence on 'Enabled' — so the transient states must stay observable while the settled ones become reachable to a polling client. Do not resolve this by deleting the transient statuses.

Traces: ds.ldaps-enable-disable-server-enforced (no frame; "Check timed out after 60.0s").

## Evidence
Suite simparity-behavioral-ga-prod-v1 — total 14/66 passed (21.2%); ds 12/51 (24%), 39 failed; ds__ec2 2/13 (15%), 11 failed. Every other ds / ds__ec2 failure in this run was marked ALREADY FILED (unimplemented DescribeCAEnrollmentPolicy, DescribeDomainControllers, ec2 CreateInternetGateway) or OWNED by an open issue and was skipped. The two below are the only surviving ones.

Cause 1 — [mismatch] ds__ec2.e2e-managed-ad-with-ldaps-and-resets-lifecycle
  Frame: File "check_ds__ec2_e2e_managed_ad_with_ldaps_and_resets_lifecycle.py", line 266, in phase -> AssertionError: [phase 2/4 solve] an Active directory serves DNS addresses [] that should sit in [IPv4Network('10.168.0.0/24'), IPv4Network('10.168.1.0/24')]
  Trajectory up to the failure all succeeded: ec2.DescribeSubnets, ds.CreateMicrosoftAD, ds.DescribeDirectories, ds.RegisterCertificate, ds.ListCertificates, ds.EnableLDAPS.
  MATCHED on the same re-read: Stage == 'Active'; (Name, ShortName, Edition, Type) == (<domain>, <short>, 'Standard', 'MicrosoftAD'); VpcSettings.SubnetIds == the two derived subnets.
  Expected: DnsIpAddrs with len == 2 and every address contained in one of the two requested subnets' CidrBlocks.
  Actual: DnsIpAddrs == [] (empty/absent).
  The two CIDRs and the subnet/VPC ids in the message are this run's CloudFormation-provisioned estate and are not requirements; the requirement is one address derived from each requested subnet.

Cause 2 — [mismatch] ds.ldaps-enable-disable-server-enforced
  Error: Check timed out after 60.0s  (no exception frame at all)
  The check: build a VPC with two subnets in two AZs; CreateMicrosoftAD(Edition='Standard'); wait for Stage 'Active' (poll interval 30 s); assert DescribeLDAPSSettings(Type='Client') shows nothing 'Enabled' (baseline); RegisterCertificate(Type='ClientLDAPS', self-signed PEM) and wait for Certificate.State in {Registered, RegisterFailed} (poll interval 10 s), asserting 'Registered'; EnableLDAPS(Type='Client') and wait for LDAPSStatus in {Enabled, EnableFailed} (poll interval 20 s), asserting 'Enabled'; DisableLDAPS(Type='Client') and wait for LDAPSStatus 'Disabled' (poll interval 15 s).
  No call in that sequence returned an error code (a refusal would have produced a ClientError frame, as it did in this suite for the stubbed operations) — the check is parked in one of the three wait loops.
  Checkout corroboration: crate::state::TRANSITION_SPAN = Duration::from_millis(500) (src/state.rs:80); advance_clock settles directory stage Requested→Active, certificate Registering→Registered and LDAPS Enabling→Enabled only once that span is due (src/state.rs ~1945-2012); enable_ldaps.rs records LDAPS_STATUS_ENABLING with status_due_at = now + TRANSITION_SPAN; register_certificate.rs records CERTIFICATE_STATE_REGISTERING with state_due_at = now + TRANSITION_SPAN. Sum of the three first-miss poll intervals (30 + 10 + 20) = 60 s, exactly the reported cap.
  No account-specific or live-only value is involved: both checks build their own network estate, and directory ids, certificate ids, addresses, request ids and timestamps differ per run and are not requirements.

## Steps to Reproduce
Boot the emulator on fresh state and point botocore clients at its ds and ec2 endpoints (plus cloudformation for the e2e estate).

Cause 1 (empty DnsIpAddrs on an Active directory):
1. Create a VPC with (at least) two subnets in two different Availability Zones; note each subnet's CidrBlock.
2. ds.create_microsoft_ad(Name='<unique>.test', ShortName='<SHORT>', Password=<strong>, Edition='Standard', VpcSettings={'VpcId': <vpc>, 'SubnetIds': [<subnetA>, <subnetB>]}).
3. Poll ds.describe_directories(DirectoryIds=[<id>]) until DirectoryDescriptions[0]['Stage'] == 'Active'.
4. Read DirectoryDescriptions[0].get('DnsIpAddrs', []). Expect two addresses, one inside subnetA's CIDR and one inside subnetB's CIDR; observe [].
5. Full check: replay ds__ec2.e2e-managed-ad-with-ldaps-and-resets-lifecycle — its phase 2/4 'solve' aborts at this read, so phases 3 and 4 (DisableLDAPS→Disabled, DeregisterCertificate, DeleteDirectory releasing the ENIs/security group, and the second DeleteDirectory refusal) are never reached and must be re-graded after the fix.

Cause 2 (LDAPS trajectory never converges):
1. Same VPC/subnets; ds.create_microsoft_ad(... Edition='Standard' ...).
2. Poll ds.describe_directories until Stage == 'Active', sleeping 30 s between attempts — record the Stage each attempt returns.
3. ds.describe_ldaps_settings(DirectoryId=<id>, Type='Client') -> expect no entry with LDAPSStatus 'Enabled'.
4. ds.register_certificate(DirectoryId=<id>, CertificateData=<self-signed PEM>, Type='ClientLDAPS'); poll ds.describe_certificate(DirectoryId=<id>, CertificateId=<id>) until Certificate.State is 'Registered' or 'RegisterFailed', sleeping 10 s between attempts — record the State each attempt returns. Expect 'Registered'.
5. ds.enable_ldaps(DirectoryId=<id>, Type='Client'); poll ds.describe_ldaps_settings(DirectoryId=<id>, Type='Client') until LDAPSSettingsInfo[0]['LDAPSStatus'] is 'Enabled' or 'EnableFailed', sleeping 20 s between attempts — record the status each attempt returns. Expect 'Enabled'.
6. ds.disable_ldaps(DirectoryId=<id>, Type='Client'); poll the same read until LDAPSStatus == 'Disabled'. Expect 'Disabled'.
7. Observe: the whole sequence does not complete inside the 60 s harness budget and the check is killed with no frame. Steps 2, 4 and 5 are where the elapsed time goes — record which of them is still returning its transient value ('Requested', 'Registering', 'Enabling') on its first attempt, then fix that gate's convergence without removing the transient states the two related open issues require.
