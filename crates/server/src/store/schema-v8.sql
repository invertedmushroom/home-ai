-- Cleanup identity outlives deleted workspace history. A late Docker create can
-- leave a stopped container even after its terminal receipt was removed.
CREATE TABLE execution_cleanup_ids (id TEXT PRIMARY KEY);
PRAGMA user_version=8;
