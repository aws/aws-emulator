Issue-Source: trace-sweep-divergence
Issue-Trace: healthlake-agent-conversation-refusals
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: healthlake-agent-conversation-refusals
Issue-Shape-Id: com.amazonaws.healthlake#HealthLake
Issue-Key: healthlake|UpdateProfileWithAgent|4xx->2xx|$.Message
Issue-Key: healthlake|UpdateProfileWithAgent|4xx->2xx|$.__type
Issue-Key: healthlake|UpdateProfileWithAgent|4xx->2xx|$headers.content-type
Issue-Key: healthlake|UpdateProfileWithAgent|4xx->2xx|status
Issue-Commit: 65b4e45f54c45e7e9fcce9255bb6d1e04dbae5a5

# HealthLake UpdateProfileWithAgent mints an unknown ConversationId instead of refusing it

> _Note: this issue was identified against the code base at commit `65b4e45f54c45e7e9fcce9255bb6d1e04dbae5a5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateProfileWithAgent

## Description
Cause 1 of 1: UpdateProfileWithAgent accepts a ConversationId that names no stored conversation and answers a full agent event stream, instead of refusing the turn.

AWS: recorded behavior only for the refusal's envelope — the Smithy model declares ResourceNotFoundException for UpdateProfileWithAgent (Smithy: UpdateProfileWithAgent errors AccessDeniedException/InternalServerException/ResourceNotFoundException/ThrottlingException/ValidationException; ConversationId -> ConversationIdString, @length 1..36, optional; awsJson1_0, host prefix datatransformation.), but it does not state the status or the message for an unknown conversation. The recorded response for a turn naming a ConversationId that does not exist is HTTP 400 with content-type application/x-amz-json-1.0 and a JSON error body carrying __type "com.amazonaws.colossus#ResourceNotFoundException" and Message "Conversation not found: <conversationId>". In that message the fixed text is "Conversation not found: " and the only variable part is the conversationId taken verbatim from the request; the recorded value sim2real-conv-f46555a9229e is one run's request value and is not part of the requirement. Note the recorded __type namespace for this agent endpoint is com.amazonaws.colossus, not com.amazonaws.healthlake, so emitting the locally modelled namespace would still diverge.

Emulator: answers HTTP 200 with content-type application/vnd.amazon.eventstream and a framed AgentOutputMessageStream (ProgressEvent / TextMessageEvent / EndOfOutputEvent); no __type and no Message are present at all. The handler resolves only the profile and then calls the conversation store with the requested id, which MINTS a fresh conversation for an id it does not hold rather than reporting the id as missing, so the turn is served — and a confirmation_response turn can go on to write the profile DRAFT against a conversation that never existed.

Why: a client that holds a stale, expired or swept ConversationId gets a success and a brand-new conversation instead of the 400 ResourceNotFoundException AWS returns. SDK error handling never fires, retry/refresh logic that keys on ResourceNotFoundException never runs, the caller believes it is continuing a multi-turn conversation whose state is actually empty, and any error-path test driven against the emulator passes a case AWS rejects. The content-type also flips from the JSON error envelope to an event stream, so an SDK tries to parse frames where it should parse an error.

Fix: the UpdateProfileWithAgent handler and the conversation lookup in the service's state — an explicitly supplied ConversationId must be resolved against the stored conversations (after retention sweeping) and a miss must refuse with ResourceNotFoundException worded as above, with minting reserved for a request that carries NO ConversationId; the error must be serialized through the JSON error envelope so the status, __type and content-type match the recording.

Traces: healthlake-agent-conversation-refusals#task#2 (expected 400 ResourceNotFoundException, got 200 event stream). Code: services/service-com-amazonaws-healthlake-healthlake/src/ops/update_profile_with_agent.rs lines 228-240 (profile existence is checked, the conversation is not; store.agent_conversation_for(input.conversation_id.as_deref(), now) mints on miss).

## Evidence
Fixture healthlake-agent-conversation-refusals, step task#2 (operation UpdateProfileWithAgent).
Expected (recorded AWS): status 400; headers content-type: application/x-amz-json-1.0; body $.__type = "com.amazonaws.colossus#ResourceNotFoundException"; body $.Message = "Conversation not found: <conversationId from request>".
Actual (emulator): status 200; headers content-type: application/vnd.amazon.eventstream; $.__type missing; $.Message missing.
Replay detail verbatim: "status: expected 400, got 200 / $.__type: missing (expected 'com.amazonaws.colossus#ResourceNotFoundException', got None) / $.Message: missing (expected 'Conversation not found: sim2real-conv-f46555a9229e', got None) / $headers.content-type: value_mismatch (expected 'application/x-amz-json-1.0', got 'application/vnd.amazon.eventstream')". Divergence key: service=healthlake|UpdateProfileWithAgent|status=400->200|keys=$.Message,$.__type,$headers.content-type,status.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay fixture healthlake-agent-conversation-refusals (step task#2).
2. Equivalently, by hand: create a data transformation profile so its ProfileId exists, then POST an awsJson1_0 UpdateProfileWithAgent request (X-Amz-Target: UpdateProfileWithAgent, host prefix datatransformation.) with that ProfileId, an InputMessage of Type "normal", and a ConversationId that was never returned by any prior turn (e.g. an arbitrary 1..36 character string).
3. Observe the emulator answers 200 with an application/vnd.amazon.eventstream body; AWS answers 400 application/x-amz-json-1.0 with __type com.amazonaws.colossus#ResourceNotFoundException and Message "Conversation not found: <that conversationId>".
