ALTER TABLE proposals ADD COLUMN rejection_reason TEXT
    CHECK(rejection_reason IS NULL OR
        (status='rejected' AND length(CAST(rejection_reason AS BLOB)) BETWEEN 1 AND 4096));
ALTER TABLE runs ADD COLUMN context_messages TEXT NOT NULL DEFAULT '[]';
PRAGMA user_version=6;
