CREATE TABLE IF NOT EXISTS qoder_campaign_operations (
 attempt_id TEXT PRIMARY KEY NOT NULL CHECK(length(attempt_id)>0),
 subject TEXT NOT NULL CHECK(length(subject)>0),
 region TEXT NOT NULL CHECK(region='international'),
 campaign_id TEXT NOT NULL CHECK(length(campaign_id)>0),
 campaign_key TEXT NOT NULL CHECK(length(campaign_key)>0),
 window_start INTEGER NOT NULL CHECK(window_start>=0),
 window_end INTEGER NOT NULL CHECK(window_end>window_start),
 credential_generation TEXT NOT NULL CHECK(length(credential_generation)>0),
 result TEXT NOT NULL CHECK(result IN ('unknown','claimed','replayed','claimed_delta_unknown')),
 UNIQUE(subject,region));
