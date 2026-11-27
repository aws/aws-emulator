Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: braket.braket-search-devices-no-filters-lists-catalog
Issue-Shape-Id: com.amazonaws.braket#Braket
Issue-Key: braket.braket-search-devices-no-filters-lists-catalog|run#6|AssertionError|assert device["deviceStatus"] in _VALID_STATUSES, (
Issue-Commit: 723e24d0ff9083736505f1fa49c337be40d1d9c1

# Braket SearchDevices publishes a device whose deviceStatus is PAUSED, a status the published catalog never reports

> _Note: this issue was identified against the code base at commit `723e24d0ff9083736505f1fa49c337be40d1d9c1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SearchDevices

## Description
Cause 1 of 1: `SearchDevices` with an empty `filters` list answers 200 with a catalog in which one entry (`arn:aws:braket:eu-north-1::device/qpu/iqm/Emerald`, provider `IQM`, type `QPU`) carries `deviceStatus` = `PAUSED`. The unfiltered Amazon Braket catalog only ever reports the three statuses an actual published device can be in, so a client that switches on the status of a listed device is handed a fourth value it will not have a branch for. Failing check: `braket.braket-search-devices-no-filters-lists-catalog`.

AWS: Smithy `SearchDevices` (services/service-com-amazonaws-braket-braket/smithy-model/com-amazonaws-braket-braket.json) returns `SearchDevicesResponse.devices`, a required `DeviceSummaryList` of `DeviceSummary`, whose `deviceStatus` member is a required `com.amazonaws.braket#DeviceStatus`. The enum shape admits `ONLINE | OFFLINE | RETIRED | PAUSED`, and the member's documentation says only "The status of the device" — so WHICH of those values the published catalog actually reports is recorded behavior only. The recorded unfiltered catalog reports `deviceStatus` only from `{ONLINE, OFFLINE, RETIRED}`: every entry is either taking work (`ONLINE`), temporarily not taking work within its availability window (`OFFLINE`), or withdrawn but still listed (`RETIRED`). No entry of the recorded catalog is `PAUSED`. The response is a 200 in both cases; no error code or message is involved in this divergence.

Emulator: the device inventory is a fixed seed table (`DEVICE_SEEDS` in services/service-com-amazonaws-braket-braket/src/state.rs), and the table's own doc comment states that each row's status was "chosen so every modelled `DeviceStatus` is reachable" — i.e. a status was picked to cover the enum rather than to match the catalog AWS publishes. That choice put `DEVICE_PAUSED` on the IQM Emerald row (state.rs, the `DeviceSeed` for `arn:aws:braket:eu-north-1::device/qpu/iqm/Emerald`). `SearchDevices` returns the seeded rows unmodified, so the `PAUSED` row appears in every unfiltered listing. Replay output: `AssertionError: unexpected deviceStatus 'PAUSED'` while iterating `resp["devices"]`; all the other catalog assertions in that check (unique ARNs, `deviceType` in `{QPU, SIMULATOR}`, non-empty `deviceName`/`providerName`, SV1 present, more than one provider, at least one QPU) hold.

Why: `deviceStatus` is what a Braket client uses to decide whether to submit to a device, to pick a fallback, or to tell a user a device is gone. A client (the Braket SDK, a console-like device picker, or a scheduler) that exhaustively matches the statuses the catalog can report will hit an unmatched arm — in strongly typed SDKs an unknown-variant value — for a device that real AWS would have listed as `OFFLINE` or `RETIRED`. It also makes the catalog self-inconsistent with task submission: a `PAUSED` device has no documented `CreateQuantumTask` refusal (`DeviceOfflineException` / `DeviceRetiredException` are the modeled ones), so the listing advertises a state whose submission behavior the API does not define.

Fix: the Braket device-inventory STATE, not the `SearchDevices` handler — the handler correctly returns the seeded rows. The seed row's `device_status` must be the status the published catalog reports for that device (one of `ONLINE`, `OFFLINE`, `RETIRED`), and the inventory must stop treating "every modelled enum value is reachable" as a seeding requirement. Because the same seeded status drives the `CreateQuantumTask` refusal and the `status_reason` the catalog publishes beside a non-online row, the seed and that refusal path have to be changed together.

Traces: check `braket.braket-search-devices-no-filters-lists-catalog` (`SearchDevices` with `filters=[]`).

## Evidence
Failing check: `braket.braket-search-devices-no-filters-lists-catalog` (service `braket`, group `data-plane`).
verdict: mismatch.

Call: `braket.search_devices(filters=[])` -> HTTP 200 (`POST /devices`).

Expected: every element of `devices` has `deviceStatus` in `{ONLINE, OFFLINE, RETIRED}` — the statuses the recorded unfiltered Amazon Braket catalog reports.
Actual: one element carries `deviceStatus` = `PAUSED`:
```
AssertionError: unexpected deviceStatus 'PAUSED'
  at check_braket_braket_search_devices_no_filters_lists_catalog.py:27
      assert device["deviceStatus"] in _VALID_STATUSES
```

Source of the value in the checkout:
* services/service-com-amazonaws-braket-braket/src/state.rs — `DEVICE_SEEDS` doc comment: "each row's status is a FIXED seed chosen so every modelled `DeviceStatus` is reachable".
* services/service-com-amazonaws-braket-braket/src/state.rs — the `DeviceSeed` row for `arn:aws:braket:eu-north-1::device/qpu/iqm/Emerald` (provider `IQM`, `device_type: DEVICE_TYPE_QPU`) sets `device_status: DEVICE_PAUSED`.
* services/service-com-amazonaws-braket-braket/smithy-model/com-amazonaws-braket-braket.json — `com.amazonaws.braket#DeviceStatus` values `[ONLINE, OFFLINE, RETIRED, PAUSED]`; `DeviceSummary.deviceStatus` required, doc "The status of the device."

Not the same as the open issue `20261124T121559Z-braket-getdevice-publishes-a-device-capability-document-that.md`: that one is about the `deviceCapabilities` JSON document returned by `GetDevice`; this one is the `deviceStatus` member of the `SearchDevices` catalog summaries.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Call `SearchDevices` with an empty filter list: `POST /devices` with body `{"filters": []}` (boto3: `braket.search_devices(filters=[])`).
3. The response is 200 with a `devices` list. Collect `{d["deviceStatus"] for d in devices}`.
4. Observe that the set contains `PAUSED`, contributed by the entry whose `deviceArn` is `arn:aws:braket:eu-north-1::device/qpu/iqm/Emerald`. The recorded catalog reports only `ONLINE`, `OFFLINE` and `RETIRED`, so any client branching exhaustively on the catalog's statuses has no arm for this entry.
5. Re-replay check `braket.braket-search-devices-no-filters-lists-catalog` to confirm a fix.
