-- Downgrade only a freshly created, empty current test database to deployed v24.
DROP TABLE IF EXISTS loongport_application_route_bindings;
DROP TABLE IF EXISTS loongport_routing_groups;
ALTER TABLE providers DROP COLUMN credential_revision;
ALTER TABLE loongport_vendor DROP COLUMN api_key_revision;
DROP TABLE IF EXISTS paperclip_run_bindings;
DROP TABLE IF EXISTS qoder_campaign_operations;
ALTER TABLE providers DROP COLUMN qoder_http_secret;
DROP TABLE IF EXISTS loongport_client_credentials;
ALTER TABLE proxy_request_logs DROP COLUMN client_id;
ALTER TABLE proxy_request_logs DROP COLUMN client_credential_id;
UPDATE loongport_schema_version SET version=24 WHERE id=1;
