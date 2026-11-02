CREATE TABLE image_assets_v14 (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id),
    image_job_id TEXT UNIQUE REFERENCES image_jobs(id),
    name TEXT NOT NULL,
    origin TEXT NOT NULL CHECK(origin IN ('upload','generation')),
    media_type TEXT NOT NULL CHECK(media_type IN ('image/png','image/jpeg')),
    bytes INTEGER NOT NULL CHECK(bytes BETWEEN 1 AND 8388608),
    width INTEGER NOT NULL CHECK(width BETWEEN 1 AND 2048),
    height INTEGER NOT NULL CHECK(height BETWEEN 1 AND 2048),
    sha256 TEXT NOT NULL CHECK(length(sha256)=64),
    content BLOB NOT NULL,
    created_at_ms INTEGER NOT NULL,
    CHECK(length(content)=bytes),
    CHECK((origin='upload' AND image_job_id IS NULL) OR (origin='generation' AND image_job_id IS NOT NULL))
);
INSERT INTO image_assets_v14(rowid,id,workspace_id,image_job_id,name,origin,media_type,bytes,width,height,sha256,content,created_at_ms)
    SELECT rowid,id,workspace_id,image_job_id,'image-'||id||CASE WHEN media_type='image/png' THEN '.png' ELSE '.jpg' END,
        'generation',media_type,bytes,width,height,sha256,content,created_at_ms FROM image_assets;
DROP TABLE image_assets;
ALTER TABLE image_assets_v14 RENAME TO image_assets;
ALTER TABLE revisions ADD COLUMN asset_bindings TEXT NOT NULL DEFAULT '[]';
ALTER TABLE proposals ADD COLUMN asset_changes TEXT NOT NULL DEFAULT '[]';
PRAGMA user_version=14;
