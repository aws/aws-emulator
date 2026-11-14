Issue-Source: trace-sweep-divergence
Issue-Trace: gamelift-catalog-list-locations
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: gamelift-catalog-list-locations gamelift-matchmaking-configuration-and-ticket-error-paths gamelift-ruleset-in-use-cannot-be-deleted
Issue-Shape-Id: com.amazonaws.gamelift#GameLift
Issue-Key: gamelift|ListLocations|2xx->2xx|$.Locations[*]
Issue-Key: gamelift|DeleteMatchmakingRuleSet|4xx->4xx|$.Message
Issue-Commit: 66471a963a64e2b8a9116c3903f472052675d9f3

# GameLift wire fidelity: ListLocations serves one-entry catalog and DeleteMatchmakingRuleSet not-found Message is reworded

> _Note: this issue was identified against the code base at commit `66471a963a64e2b8a9116c3903f472052675d9f3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListLocations

## Description
Two independent causes in the GameLift emulator, both response-fidelity defects on paths that already answer with the right HTTP status: the managed-location catalog ListLocations serves, and the not-found Message DeleteMatchmakingRuleSet pairs with its NotFoundException. A fixer must close BOTH before submitting.

Neither cause's replay key is carried by an existing open issue. Related but NOT covering: 20261029T183132Z-gamelift-responses-omit-modeled-fields-matchmaking-defaults-terminationtime.md describes the ListLocations catalog gap in prose but its Issue-Key list carries only the CreateMatchmakingConfiguration, DescribeFleetAttributes and CreateAlias keys, so the ListLocations cluster is unowned and still diverges; 20261103T083127Z-gamelift-refusal-fidelity-3-error-class-message-divergences.md covers DeleteMatchmakingRuleSet's IN-USE refusal message (InvalidRequestException path), not the NOT-FOUND message below; 20261029T185309Z-gamelift-deletematchmakingruleset-stub.md covers that operation's absence, while the replay shows it implemented and refusing with 400.

Cause 1 of 2 - ListLocations returns only the emulator's own region instead of the GameLift managed-location catalog, and no PingBeacon.
AWS: ListLocations with no filters returns the full set of GameLift-supported locations available to the account - AWS Regions plus local zones (the recording shows 46 entries: ap-south-1, us-west-1, us-east-1-mia-2, us-east-1-lim-1, ap-southeast-2, us-east-1-chi-2, us-west-2-phx-2, us-east-1-iah-1, eu-central-1, us-east-1-mci-1, ap-southeast-5, ...) - and every managed location carries PingBeacon.UDPEndpoint with a Domain and Port (recorded form: gamelift-ping.<region>.api.aws for a Region, gamelift-ping-<zone-code>.<region>.api.aws for a local zone, Port 7770). Smithy: ListLocations output Locations: LocationModelList, paginated (inputToken/outputToken NextToken, pageSize Limit range 1-200); LocationModel members LocationName (pattern ^[A-Za-z0-9\-]+$), LocationArn (custom locations only, pattern ^arn:.*:location\/custom-\S+$) and PingBeacon; PingBeacon.UDPEndpoint {Domain (NonZeroAndMaxString), Port (PositiveInteger)} and UDPEndpoint.Port is documented as "typically port 7770" for GameLift ping beacons. The exact membership of the catalog is environment data; a single-entry managed-location list with no PingBeacon is not AWS's shape.
Emulator: the response is exactly one element, {"LocationName": "us-west-2"} - the emulator's own region - with no PingBeacon member; all other recorded entries are absent ($.Locations[0] through $.Locations[45] reported missing/mismatched), status 200/200.
Why: a client that enumerates locations to build or validate a multi-location fleet (or to pick fleet locations by Region) sees exactly one choice and cannot place a fleet anywhere else; a client that reads PingBeacon.UDPEndpoint to measure player network latency has no endpoint to ping at all, so latency-based placement silently degrades. Pagination is also never exercised because one entry never fills a page.
Fix: the ListLocations handler's catalog source and pagination - serve the GameLift managed-location set with PingBeacon.UDPEndpoint populated for each managed location, plus the account's custom locations (those carrying LocationArn), and honour Limit/NextToken over that set.
Traces: gamelift-catalog-list-locations#task#1 (ListLocations, 200/200).

Cause 2 of 2 - DeleteMatchmakingRuleSet on a rule set that does not exist refuses with the right class but a reworded Message.
AWS: HTTP 400 with the modeled NotFoundException and Message "Matchmaking rule set <name> not found". Fixed text: "Matchmaking rule set " and " not found" - no trailing period, no "Failed to find" prefix and no colon; the ONE variable part is the rule-set name/ARN copied verbatim from the request's Name member (the recording's sim2real-probe-gamelift-missing-ruleset-<suffix> is just that request value filling the template, not part of it). Smithy: DeleteMatchmakingRuleSet models NotFoundException (client, httpError 400 defaulted, documented "The requested resource was not found. The resource was either not created yet or deleted.") alongside InvalidRequestException; input Name is required, MatchmakingRuleSetName length 1-256, pattern ^([a-zA-Z0-9-\.]*|arn:.*:matchmakingruleset\/[a-zA-Z0-9-\.]*)$, so a name that resolves to nothing is this not-found path.
Emulator: same status 400 and the same error class, but Message "Failed to find rule set: <name>" - invented fixed text ("Failed to find rule set: ") in place of AWS's "Matchmaking rule set " ... " not found" template.
Why: operators and clients that match or surface AWS's not-found text for a missing rule set (log scrapers, idempotent-delete helpers that treat the AWS message as the confirmation that the rule set is already gone, test assertions) never see it, and the text they do see is not a string AWS ever emits for this error class.
Fix: the DeleteMatchmakingRuleSet handler's rule-set-resolution failure path and the shared message helper it calls, crate::state::matchmaking_rule_set_not_found_message, which must emit AWS's template; note AWS words the same missing resource differently per operation, so the helper may need splitting per caller rather than being shared.
Traces: gamelift-matchmaking-configuration-and-ticket-error-paths#task#2 and gamelift-ruleset-in-use-cannot-be-deleted#task#3 (DeleteMatchmakingRuleSet, 400/400); handler code at services/service-com-amazonaws-gamelift-gamelift/src/ops/delete_matchmaking_rule_set.rs lines 169-178.

## Evidence
Cause 1: gamelift-catalog-list-locations#task#1 ListLocations expected 200, actual 200 - $.Locations[0] expected {'LocationName': 'ap-south-1', 'PingBeacon': {'UDPEndpoint': {'Domain': 'gamelift-ping.ap-south-1.api.aws', 'Port': 7770}}}, got {'LocationName': 'us-west-2'}; $.Locations[1] .. $.Locations[45] expected present (us-west-1, us-east-1-mia-2, us-east-1-lim-1, ap-southeast-2, us-east-1-chi-2, us-west-2-phx-2, us-east-1-iah-1, eu-central-1, us-east-1-mci-1, ap-southeast-5, ...), got None. No entry in the emulator's response carries a PingBeacon member.

Cause 2: gamelift-matchmaking-configuration-and-ticket-error-paths#task#2 DeleteMatchmakingRuleSet expected 400, actual 400 - $.Message expected 'Matchmaking rule set <name> not found', got 'Failed to find rule set: <name>' (recorded with <name> = sim2real-probe-gamelift-missing-ruleset-ebd77c6e, the request's Name value). Same divergence on gamelift-ruleset-in-use-cannot-be-deleted#task#3. HTTP status and error class matched; only the Message fixed text diverges.

## Steps to Reproduce
1. Boot the emulator and replay the wire corpus (simparity-wire-aws-v1).
2. Cause 1: call ListLocations with no Filters and no Limit; count the returned Locations and check that each managed location carries PingBeacon.UDPEndpoint with a Domain and Port. Expect the GameLift managed-location catalog (Regions plus local zones, 46 entries in the recording) with ping beacons on port 7770; the emulator returns one entry {"LocationName": "<own region>"} and no PingBeacon. Then re-run with Limit=1 and follow NextToken to confirm pagination over the catalog.
3. Cause 2: call DeleteMatchmakingRuleSet(Name="<a rule set that was never created>") and read the HTTP status, error code and Message. Expect 400 / NotFoundException / "Matchmaking rule set <that same Name> not found"; the emulator returns "Failed to find rule set: <Name>".
4. Re-replay gamelift-catalog-list-locations#task#1 and gamelift-matchmaking-configuration-and-ticket-error-paths#task#2 (and gamelift-ruleset-in-use-cannot-be-deleted#task#3) to confirm both causes are closed.
