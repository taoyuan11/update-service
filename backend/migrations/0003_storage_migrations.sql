CREATE TABLE storage_migrations (
    id UUID PRIMARY KEY,
    source_profile_id UUID REFERENCES storage_profiles(id) ON DELETE SET NULL,
    destination_profile_id UUID REFERENCES storage_profiles(id) ON DELETE SET NULL,
    source_profile_name VARCHAR(100) NOT NULL,
    destination_profile_name VARCHAR(100) NOT NULL,
    source_backend VARCHAR(16) NOT NULL,
    destination_backend VARCHAR(16) NOT NULL,
    status VARCHAR(24) NOT NULL DEFAULT 'queued'
        CHECK (status IN ('queued', 'running', 'completed', 'partial_failed', 'cancelled')),
    total_objects BIGINT NOT NULL DEFAULT 0 CHECK (total_objects >= 0),
    completed_objects BIGINT NOT NULL DEFAULT 0 CHECK (completed_objects >= 0),
    failed_objects BIGINT NOT NULL DEFAULT 0 CHECK (failed_objects >= 0),
    skipped_objects BIGINT NOT NULL DEFAULT 0 CHECK (skipped_objects >= 0),
    total_bytes BIGINT NOT NULL DEFAULT 0 CHECK (total_bytes >= 0),
    completed_bytes BIGINT NOT NULL DEFAULT 0 CHECK (completed_bytes >= 0),
    cancel_requested BOOLEAN NOT NULL DEFAULT FALSE,
    last_error TEXT,
    requested_by UUID NOT NULL REFERENCES users(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    started_at TIMESTAMPTZ,
    finished_at TIMESTAMPTZ,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE UNIQUE INDEX one_active_storage_migration
    ON storage_migrations ((TRUE))
    WHERE status IN ('queued', 'running');
CREATE INDEX storage_migrations_created_idx ON storage_migrations(created_at DESC);

CREATE TABLE storage_migration_items (
    id UUID PRIMARY KEY,
    migration_id UUID NOT NULL REFERENCES storage_migrations(id) ON DELETE CASCADE,
    artifact_id UUID REFERENCES artifacts(id) ON DELETE SET NULL,
    artifact_id_snapshot UUID NOT NULL,
    object_key TEXT NOT NULL,
    size_bytes BIGINT NOT NULL CHECK (size_bytes >= 0),
    sha256 CHAR(64) NOT NULL,
    content_type VARCHAR(255) NOT NULL,
    status VARCHAR(16) NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'running', 'succeeded', 'failed', 'skipped')),
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    bytes_copied BIGINT NOT NULL DEFAULT 0 CHECK (bytes_copied >= 0),
    next_attempt_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_error TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    started_at TIMESTAMPTZ,
    finished_at TIMESTAMPTZ,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (migration_id, artifact_id_snapshot)
);
CREATE INDEX storage_migration_items_ready_idx
    ON storage_migration_items(migration_id, status, next_attempt_at);
