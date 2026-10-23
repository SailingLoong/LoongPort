-- Frozen published schema28 client shape. No CN receipt migration is implied.
CREATE TABLE loongport_client_credentials (
    id TEXT PRIMARY KEY NOT NULL,
    client_id TEXT NOT NULL,
    route_app TEXT NOT NULL,
    allowed_models TEXT NOT NULL,
    secret TEXT NOT NULL,
    secret_digest TEXT NOT NULL UNIQUE,
    revoked INTEGER NOT NULL DEFAULT 0 CHECK(revoked IN (0, 1))
);
ALTER TABLE proxy_request_logs ADD COLUMN client_id TEXT;
ALTER TABLE proxy_request_logs ADD COLUMN client_credential_id TEXT;
