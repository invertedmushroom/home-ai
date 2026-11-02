ALTER TABLE image_jobs ADD COLUMN progress TEXT;
ALTER TABLE image_jobs ADD COLUMN last_contact_at_ms INTEGER;
PRAGMA user_version=15;
