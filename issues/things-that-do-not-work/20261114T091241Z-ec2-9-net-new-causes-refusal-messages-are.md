Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.copy-volumes-undersized-target-rejected ec2.tgwc-duplicate-transit-gateway-static-route-rejected ec2.op-contract-delete-fpga-image-rejected ec2.op-contract-cancel-capacity-reservation-fleets ec2.op-contract-describe-coip-pools ec2.op-contract-acceptVpcEndpointConnections-b ec2.op-contract-modify-volume ec2.op-contract-enable-volume-io ec2.op-contract-provision-byoip-cidr-rejected
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.copy-volumes-undersized-target-rejected|run#12|AssertionError|assert message == (
Issue-Key: ec2.tgwc-duplicate-transit-gateway-static-route-rejected|run#12|AssertionError|assert message == expected, f"a duplicate 10.137.0.0/24 answered message {message!r}"
Issue-Key: ec2.op-contract-delete-fpga-image-rejected|run#12|AssertionError|assert not_found_message == f"Image ID '{unassigned_id}' not found.", not_found_message
Issue-Key: ec2|CancelCapacityReservationFleets|InvalidCapacityReservationFleetId.NotFound
Issue-Key: ec2.op-contract-describe-coip-pools|run#11|AssertionError|assert token_code_msg == f"Next token '{bogus_token}' is invalid", (
Issue-Key: ec2.op-contract-acceptVpcEndpointConnections-b|run#16|AssertionError|assert info3["message"] == malformed_message, (
Issue-Key: ec2.op-contract-modify-volume|run#19|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: ec2.op-contract-enable-volume-io|run#12|AssertionError|assert malformed_code == "InvalidParameterValue", f"expected InvalidParameterValue, got {malformed_code!r}"
Issue-Key: ec2.op-contract-provision-byoip-cidr-rejected|run#10|AssertionError|assert err2 == "MissingParameter", f"expected MissingParameter, got {err2}"
Issue-Commit: 965b3053fd6a88603311dd441a6858d4fb538678

# EC2: 9 net-new causes — refusal messages are emulator-authored prose and three refusals carry the wrong error code (CopyVolumes, CreateTransitGatewayRoute, DeleteFpgaImage, CancelCapacityReservationFleets, DescribeCoipPools, AcceptVpcEndpointConnections, ModifyVolume, EnableVolumeIO, ProvisionByoipCidr)

> _Note: this issue was identified against the code base at commit `965b3053fd6a88603311dd441a6858d4fb538678`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CopyVolumes

## Description
Nine net-new root causes in the ec2 control plane. In each one the emulator refuses the request — so the refusal itself is right — but the message is emulator-authored prose rather than AWS's wording, and in three of them the error code or class is wrong too. An HTTP status, an error code and an error message are requirements, so each is a defect. The EC2 Smithy model declares errors: [] for all of these operations (ec2Query carries error codes at the protocol level), so each requirement below is AWS's documented/recorded refusal. Each cause must be closed separately.

Cause 1 of 9: CopyVolumes with a Size below the source volume's.
AWS: HTTP 400, InvalidParameterValue, message "The size of the volume copy must be greater than or equal to the source volume. Specify a larger volume size and try again." — entirely fixed text, no request values; no copy is created.
Emulator: same code, message "Invalid value '1' for size. The size must be equal to or greater than the size of the source volume, which is 2 GiB" (emulator prose that interpolates the requested size and the source size).

Cause 2 of 9: CreateTransitGatewayRoute for a CIDR already present in the table.
AWS: HTTP 400, RouteAlreadyExists, message "Route <destination CIDR> already exists in Transit Gateway Route Table <tgw-rtb id>." — fixed text with the request's DestinationCidrBlock and TransitGatewayRouteTableId substituted.
Emulator: same code, message "The route identified by <cidr> already exists." — different sentence and the route table is not named.

Cause 3 of 9: DeleteFpgaImage with an unassigned 8-hex-digit AFI id.
AWS: HTTP 400, InvalidFpgaImageID.NotFound, message "Image ID '<afi-id>' not found." — fixed text with the requested id.
Emulator: message "The fpga-image ID '<afi-id>' does not exist".

Cause 4 of 9: CancelCapacityReservationFleets with an unknown fleet id.
AWS: the per-fleet failure carries the message "Capacity Reservation Fleet ID '<crf-id>' is not found" — fixed text with the requested id.
Emulator: "The Capacity Reservation Fleet ID '<crf-id>' does not exist".

Cause 5 of 9: DescribeCoipPools with an invalid NextToken.
AWS: HTTP 400, InvalidPaginationToken, message "Next token '<token>' is invalid" — fixed text with the token from the request.
Emulator: message "The passed pagination token is invalid" — the offending token is not quoted.

Cause 6 of 9: AcceptVpcEndpointConnections with a malformed ServiceId.
AWS: the malformed-id refusal message is "Invalid Id: '<value>' (expecting 'vpce-svc-...; the Id may only contain lowercase alphanumeric characters and two dashes')" — fixed text with the rejected value.
Emulator: 'Invalid id: "<value>"' — different wording, different quoting, and the expected-shape clause is dropped.

Cause 7 of 9: ModifyVolume with a fabricated but well-formed-looking volume id.
AWS: validates the id BEFORE any existence check and answers HTTP 400 InvalidParameterValue with the message "Value ( <volume id> ) for parameter VOLUME is invalid. " (note AWS's spacing, and the trailing space).
Emulator: answers the "malformed id" class instead — message "The volume ID '<volume id>' is malformed".

Cause 8 of 9: EnableVolumeIO with a non-EC2-shaped volume id.
AWS: HTTP 400, error code InvalidParameterValue, message "Value (<value>) for parameter volumeId is invalid. Expected: 'vol-...'."; a well-formed but nonexistent volume id draws a DIFFERENT class on this operation (InternalError, "An internal error has occurred"), so the two input faults are distinguishable.
Emulator: answers code InvalidVolumeID.Malformed for the malformed id, collapsing AWS's parameter-validation class into the id class.

Cause 9 of 9: ProvisionByoipCidr with a valid CIDR and no CidrAuthorizationContext.
AWS: HTTP 400, error code MissingParameter, message "You must provide a CidrAuthorizationContext parameter." The malformed-CIDR leg (InvalidParameterValue, "The CIDR (<cidr>) is not valid.") already matches, so the two faults are distinct classes. Smithy: CidrAuthorizationContext is modelled optional, so this is a service-side required-together rule.
Emulator: answers InvalidParameterValue for the missing-context case, so both legs collapse into one class.

Why (all nine): clients match on the error code and, for user-facing tooling, on the message; AWS CLI/SDK error handling, retry classification and log-based alerting all key off them. A wrong code sends a caller down the wrong branch (parameter validation vs id malformed vs missing parameter), and emulator-authored prose breaks any assertion or translation of the documented text. Omitting the offending value (causes 2 and 5) also removes the only information a caller needs to fix the request.
Fix: each operation's refusal path under services/service-com-amazonaws-ec2-amazonec2/src/ops/ — the message template, and for causes 7-9 the classification that picks the error code before the existence check.
Traces: one check per cause, listed in trace_ids in the same order as the causes above.

## Evidence
Cause 1 — AssertionError: unexpected error message "Invalid value '1' for size. The size must be equal to or greater than the size of the source volume, which is 2 GiB"; expected "The size of the volume copy must be greater than or equal to the source volume. Specify a larger volume size and try again."
Cause 2 — AssertionError: a duplicate 10.137.0.0/24 answered message 'The route identified by 10.137.0.0/24 already exists.'; expected "Route 10.137.0.0/24 already exists in Transit Gateway Route Table <tgw-rtb-...>." (code RouteAlreadyExists matched).
Cause 3 — AssertionError: The fpga-image ID 'afi-12345678' does not exist; expected "Image ID 'afi-12345678' not found."
Cause 4 — AssertionError: The Capacity Reservation Fleet ID 'crf-00000000000000080' does not exist; expected "Capacity Reservation Fleet ID 'crf-00000000000000080' is not found".
Cause 5 — AssertionError: expected the exact InvalidPaginationToken message, got 'The passed pagination token is invalid'.
Cause 6 — AssertionError: expected message "Invalid Id: '__NOT_A_SERVICE_ID__' (expecting 'vpce-svc-...; the Id may only contain lowercase alphanumeric characters and two dashes')", got 'Invalid id: "__NOT_A_SERVICE_ID__"'.
Cause 7 — AssertionError: expected message 'Value ( vol-1234567890abcdef0 ) for parameter VOLUME is invalid. ', got "The volume ID 'vol-1234567890abcdef0' is malformed".
Cause 8 — AssertionError: expected InvalidParameterValue, got 'InvalidVolumeID.Malformed' (expected message "Value (not-a-real-volume-id) for parameter volumeId is invalid. Expected: 'vol-...'.").
Cause 9 — AssertionError: expected MissingParameter, got InvalidParameterValue (expected message "You must provide a CidrAuthorizationContext parameter.").

## Steps to Reproduce
1. CreateVolume(gp3, Size=2) then CopyVolumes(SourceVolumeId=that, Size=1) — compare message.
2. CreateTransitGateway + two TGW route tables; CreateTransitGatewayRoute(10.137.0.0/24, Blackhole) twice in one table — compare the RouteAlreadyExists message.
3. DeleteFpgaImage(FpgaImageId='afi-12345678').
4. CancelCapacityReservationFleets with an unknown crf- id.
5. DescribeCoipPools(NextToken='<garbage>').
6. AcceptVpcEndpointConnections(ServiceId='__NOT_A_SERVICE_ID__').
7. ModifyVolume(VolumeId='vol-1234567890abcdef0', Size=...).
8. EnableVolumeIO(VolumeId='not-a-real-volume-id') and EnableVolumeIO(VolumeId='vol-00000000000000000').
9. ProvisionByoipCidr(Cidr='999.999.999.0/24') and ProvisionByoipCidr(Cidr='198.51.100.0/24') with no CidrAuthorizationContext.
Replay the trace_ids listed.
