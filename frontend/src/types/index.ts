export type Role = 'admin' | 'user'
export type ReleaseStatus = 'draft' | 'published' | 'withdrawn'

export interface User { id: string; username: string; role: Role; enabled: boolean; created_at: string }
export interface App { id: string; name: string; description: string; owner_id: string; status: 'active' | 'deleted'; created_at: string; updated_at: string }
export interface Artifact { id: string; release_id: string; platform: string; original_file_name: string; content_type: string; size_bytes: number; sha256: string; storage_profile_id: string; object_key: string; created_at: string }
export interface Release { id: string; app_id: string; version: string; channel: 'stable' | 'beta'; release_notes: string; status: ReleaseStatus; published_at: string | null; created_at: string; updated_at: string }
export interface ReleaseDetail extends Release { artifacts: Artifact[] }
export interface StorageProfile { id: string; name: string; backend: 'local' | 's3'; config: Record<string, unknown>; has_secret: boolean; is_active: boolean; created_by: string; created_at: string; artifact_count: number; artifact_bytes: number }
export type StorageMigrationStatus = 'queued' | 'running' | 'completed' | 'partial_failed' | 'cancelled'
export type StorageMigrationItemStatus = 'pending' | 'running' | 'succeeded' | 'failed' | 'skipped'
export interface StorageMigration {
  id: string
  source_profile_id: string | null
  destination_profile_id: string | null
  source_profile_name: string
  destination_profile_name: string
  source_backend: 'local' | 's3' | string
  destination_backend: 'local' | 's3' | string
  status: StorageMigrationStatus
  total_objects: number
  completed_objects: number
  failed_objects: number
  skipped_objects: number
  total_bytes: number
  completed_bytes: number
  cancel_requested: boolean
  last_error: string | null
  requested_by: string
  created_at: string
  started_at: string | null
  finished_at: string | null
  updated_at: string
}
export interface StorageMigrationItem {
  id: string
  migration_id: string
  artifact_id: string | null
  artifact_id_snapshot: string
  object_key: string
  size_bytes: number
  sha256: string
  content_type: string
  status: StorageMigrationItemStatus
  attempts: number
  bytes_copied: number
  next_attempt_at: string
  last_error: string | null
  created_at: string
  started_at: string | null
  finished_at: string | null
  updated_at: string
}
export interface StorageMigrationDetail extends StorageMigration { failed_items: StorageMigrationItem[] }
export interface Page<T> { items: T[]; total: number; page: number; page_size: number }
export interface AuthResponse { user: User; csrf_token: string }
