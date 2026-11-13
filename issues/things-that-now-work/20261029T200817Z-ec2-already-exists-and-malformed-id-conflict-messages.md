Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-launch-template-versioning
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-launch-template-versioning ec2-placement-group-and-tagging ec2-security-group-rule-lifecycle ec2-on-demand-capacity-reservation-visibility
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|CreateLaunchTemplate|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|CreatePlacementGroup|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|CreateSecurityGroup|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|AuthorizeSecurityGroupIngress|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|DescribeCapacityReservationCancellationQuotes|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|DescribeCapacityReservationTopology|4xx->4xx|$.Response.Errors.Error.Message
Issue-Commit: c42b5bef1562db44010b2579a8522ff39a3564f6

# EC2 already-exists and malformed-id conflict messages are reworded, dropping values AWS includes

> _Note: this issue was identified against the code base at commit `c42b5bef1562db44010b2579a8522ff39a3564f6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateLaunchTemplate

## Description
Six net-new causes, one defect: on duplicate-resource and malformed-id refusals the emulator's message text differs from AWS's - it adds a value AWS leaves out, omits a value AWS includes, or identifies the conflicting object by an internal id instead of its describing attributes. Status and error code match in every case. ec2 models no error shapes in Smithy, so the expectations are recorded behavior only, read off the recorded 400s.

Cause 1 of 6 - CreateLaunchTemplate with a name already in use (ec2-launch-template-versioning, task#6). AWS: 'Launch template name already in use.' - fixed text, the name is NOT interpolated. Emulator: "The launch template name '<name>' is already in use". Fix: the duplicate-name branch of CreateLaunchTemplate's validation.

Cause 2 of 6 - CreatePlacementGroup for an existing group (ec2-placement-group-and-tagging#task#3). AWS: "The placement group '<group-name>' already exists." - note the terminating period. Emulator omits it: "The placement group '<group-name>' already exists".

Cause 3 of 6 - CreateSecurityGroup with a duplicate name in a VPC (ec2-security-group-rule-lifecycle#task#12). AWS: "The security group '<group-name>' already exists for VPC '<vpc-id>'" - the VPC id comes from the request/state. Emulator truncates the template and emits no vpc id: "The security group '<group-name>' already exists for VPC".

Cause 4 of 6 - AuthorizeSecurityGroupIngress with a duplicate rule (ec2-security-group-rule-lifecycle#task#5). AWS identifies the rule by its permission tuple: 'the specified rule "peer: <cidr>, <protocol>, from port: <from>, to port: <to>, <ALLOW|DENY>" already exists', with every field taken from the request. Emulator substitutes the stored rule id: 'the specified rule "<sgr-id>" already exists'. Why: a client cannot tell which of its requested permissions collided, and the rule id is a per-run generated value.

Cause 5 of 6 - DescribeCapacityReservationCancellationQuotes with a malformed quote id (ec2-on-demand-capacity-reservation-visibility#task#9). AWS: "Cancellation Quote Id '<crq-id>' is malformed." - resource label 'Cancellation Quote Id', terminating period. Emulator: "Capacity Reservation cancellation quote ID '<crq-id>' is malformed".

Cause 6 of 6 - DescribeCapacityReservationTopology with a malformed reservation id (same fixture, task#6). AWS: "Capacity Reservation Id '<cr-id>' is malformed." Emulator: "Capacity Reservation ID '<cr-id>' is malformed" - 'ID' for 'Id' and the period dropped.

Why: these strings are the only machine-readable detail on an otherwise identical error code; suites and operators match them, and causes 3 and 4 also strip information (the vpc id, the colliding permission) that a client needs to recover.

Fix: the duplicate/conflict and malformed-id message construction in each operation's validation step under services/service-com-amazonaws-ec2-amazonec2/src/ops/ (create_launch_template.rs, create_placement_group.rs, create_security_group.rs, authorize_security_group_ingress.rs, describe_capacity_reservation_cancellation_quotes.rs, describe_capacity_reservation_topology.rs).

Traces: representative step plus the five fixture#step bindings in trace_ids.

## Evidence
Replay detail, verbatim (all 400 expected / 400 returned, code matching):
1. CreateLaunchTemplate: expected 'Launch template name already in use.'; actual "The launch template name 'sim2real-probe-ec2-lt-089649aa' is already in use".
2. CreatePlacementGroup: expected "The placement group 'sim2real-probe-ec2-pg-55184b66' already exists."; actual "The placement group 'sim2real-probe-ec2-pg-55184b66' already exists".
3. CreateSecurityGroup: expected "The security group 'sim2real-probe-ec2-sg-b3c0a614' already exists for VPC 'vpc-00000000000000022'"; actual "The security group 'sim2real-probe-ec2-sg-b3c0a614' already exists for VPC".
4. AuthorizeSecurityGroupIngress: expected 'the specified rule "peer: 10.0.0.0/8, TCP, from port: 8080, to port: 8080, ALLOW" already exists'; actual 'the specified rule "sgr-00000000000000037" already exists'.
5. DescribeCapacityReservationCancellationQuotes: expected "Cancellation Quote Id 'crq-00000000000000000' is malformed."; actual "Capacity Reservation cancellation quote ID 'crq-00000000000000000' is malformed".
6. DescribeCapacityReservationTopology: expected "Capacity Reservation Id 'cr-00000000000000000' is malformed."; actual "Capacity Reservation ID 'cr-00000000000000000' is malformed".
All six clusters: verdict=mismatch, stub_path empty, owned_by empty.

## Steps to Reproduce
1. Clone, boot the emulator built from the checkout.
2. replay_wire_corpus on simparity-wire-aws-v1.
3. For ec2-launch-template-versioning step task#6 and each fixture#step in trace_ids, diff $.Response.Errors.Error.Message against the recorded 400.
