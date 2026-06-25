# cases

On-disk Case stubs for when the Cases MCP backend isn't running: file the Case here, then promote it
via `case_create` when the backend is back. Once the headless Cases backend lands (data model →
Cases API/MCP), this mirrors the DB-backed Cases; until then a spec in `docs/specs/` doubles as the
Case of record.
