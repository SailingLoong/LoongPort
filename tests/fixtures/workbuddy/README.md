# WorkBuddy serialized account fixture

`synthetic-account-views.json.txt` preserves the 2631 source bytes from Library
`libfile_198bd01aeea48191895778794987425e`, version 0, file
`file_00000000287481f9800272f88cdd3410`, originally `synthetic-account-views.json`.
SHA-256: `81b148b05547becdfcfa8ffccb180fa106adb2732c075319006dd3e08b0d6075`.
The `.txt` suffix preserves the original JSON byte-for-byte, including its lack of
a trailing newline, when the repository JSON formatter runs. Tests import it as
raw text and parse JSON without reserializing it.

The WorkBuddy owner exported these views through the actual `Engine.claim/list`,
encrypted account store, and `AccountView` serde path with synthetic network
responses. This frontend test consumes that output; it does not rerun the backend
producer or use a real account.

- `known`: generated at Unix milliseconds `1800000000000`; confirmed claim 20,
  earliest known credit expiry at `1800000010000` with 30 remaining, unit
  `workbuddy:credits`.
- `unknown`: read at `1800000000000`, no confirmed claim or known expiry.
- `stale`: queried at `1800000020000`, keeps the confirmed claim and marks expiry
  stale with a null `nextKnownCreditExpiry`.

The integration tests keep these source dates unchanged and control only their
presentation clock. They exercise the original IPC adapter, account inventory
projection, QueryClient and mounted ServicesPage/AccountOverviewRow, including
refresh from the known view to the stale view.
