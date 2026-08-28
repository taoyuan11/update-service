CREATE TABLE storage_cleanup_jobs (
    id UUID PRIMARY KEY,
    storage_profile_id UUID NOT NULL REFERENCES storage_profiles(id),
    object_key TEXT NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 0,
    next_attempt_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_error TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX storage_cleanup_jobs_ready_idx ON storage_cleanup_jobs(next_attempt_at);

