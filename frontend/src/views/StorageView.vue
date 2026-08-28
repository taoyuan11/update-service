<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { ElMessage, ElMessageBox } from 'element-plus'
import { Plus, Refresh } from '@element-plus/icons-vue'
import { api } from '@/api/client'
import type { Page, StorageMigration, StorageMigrationDetail, StorageProfile } from '@/types'

const profiles = ref<StorageProfile[]>([])
const migrations = ref<StorageMigration[]>([])
const loading = ref(false)
const dialog = ref(false)
const migrationDialog = ref(false)
const detailDialog = ref(false)
const saving = ref(false)
const migrationSaving = ref(false)
const detail = ref<StorageMigrationDetail>()
const form = ref(freshForm())
const migrationForm = ref({ source_profile_id: '', destination_profile_id: '' })
let pollTimer: ReturnType<typeof setInterval> | undefined

function freshForm() {
  return {
    name: '',
    backend: 'local' as 'local' | 's3',
    root: '/var/lib/update-service',
    endpoint: '',
    region: 'us-east-1',
    bucket: '',
    prefix: '',
    path_style: true,
    access_key: '',
    secret_key: '',
  }
}

const isS3 = computed(() => form.value.backend === 's3')
const sourceProfile = computed(() => profiles.value.find((profile) => profile.id === migrationForm.value.source_profile_id))
const destinationProfile = computed(() => profiles.value.find((profile) => profile.id === migrationForm.value.destination_profile_id))
const migrationDirection = computed(() => {
  if (!sourceProfile.value || !destinationProfile.value) return ''
  return `${backendLabel(sourceProfile.value.backend)} → ${backendLabel(destinationProfile.value.backend)}`
})
const migrationAllowed = computed(() => {
  const source = sourceProfile.value?.backend
  const destination = destinationProfile.value?.backend
  return Boolean(source && destination && sourceProfile.value?.id !== destinationProfile.value?.id && !(source === 'local' && destination === 'local'))
})
const activeMigration = computed(() => migrations.value.find((migration) => ['queued', 'running'].includes(migration.status)))

function statusLabel(status: StorageMigration['status']) {
  return ({ queued: '排队中', running: '迁移中', completed: '已完成', partial_failed: '部分失败', cancelled: '已取消' })[status]
}
function statusType(status: StorageMigration['status']) {
  return ({ queued: 'info', running: '', completed: 'success', partial_failed: 'danger', cancelled: 'warning' }[status] ?? 'info') as 'success' | 'warning' | 'info' | 'danger' | ''
}
function backendLabel(backend: string) { return backend === 's3' ? 'S3' : '本地存储' }
function formatBytes(value: number) {
  if (!Number.isFinite(value) || value <= 0) return '0 B'
  const units = ['B', 'KB', 'MB', 'GB', 'TB']
  const index = Math.min(Math.floor(Math.log(value) / Math.log(1024)), units.length - 1)
  return `${(value / 1024 ** index).toFixed(index === 0 ? 0 : 2)} ${units[index]}`
}
function progress(migration: StorageMigration) {
  if (!migration.total_objects) return migration.status === 'completed' ? 100 : 0
  return Math.min(100, Math.round(((migration.completed_objects + migration.failed_objects) / migration.total_objects) * 100))
}
function isTerminal(status: StorageMigration['status']) { return ['completed', 'partial_failed', 'cancelled'].includes(status) }

async function loadProfiles() {
  profiles.value = (await api<Page<StorageProfile>>('/storage-profiles')).items
}
async function loadMigrations() {
  migrations.value = (await api<Page<StorageMigration>>('/storage-migrations')).items
}
async function load() {
  loading.value = true
  try {
    await Promise.all([loadProfiles(), loadMigrations()])
    if (activeMigration.value) startPolling(activeMigration.value.id)
  } catch (error) {
    ElMessage.error(error instanceof Error ? error.message : '加载失败')
  } finally { loading.value = false }
}

function openCreate() { form.value = freshForm(); dialog.value = true }
async function create() {
  const f = form.value
  const config = f.backend === 'local'
    ? { root: f.root }
    : { endpoint: f.endpoint || null, region: f.region, bucket: f.bucket, prefix: f.prefix, path_style: f.path_style }
  const secret = f.backend === 's3' ? JSON.stringify({ access_key: f.access_key, secret_key: f.secret_key }) : undefined
  saving.value = true
  try {
    await api<StorageProfile>('/storage-profiles', { method: 'POST', body: JSON.stringify({ name: f.name, backend: f.backend, config, secret }) })
    dialog.value = false
    await load()
    ElMessage.success('存储配置已创建')
  } catch (error) {
    ElMessage.error(error instanceof Error ? error.message : '创建失败')
  } finally { saving.value = false }
}
async function testProfile(profile: StorageProfile) {
  try { await api<void>(`/storage-profiles/${profile.id}/test`, { method: 'POST' }); ElMessage.success('连接测试成功') }
  catch (error) { ElMessage.error(error instanceof Error ? error.message : '连接测试失败') }
}
async function activate(profile: StorageProfile) {
  try { await api<StorageProfile>(`/storage-profiles/${profile.id}/activate`, { method: 'POST' }); await loadProfiles(); ElMessage.success('已设为新的上传目标') }
  catch (error) { ElMessage.error(error instanceof Error ? error.message : '启用失败') }
}
async function remove(profile: StorageProfile) {
  try {
    await ElMessageBox.confirm('删除配置不会删除源端保留的文件，之后服务将无法通过此配置访问它们。', '删除存储配置', { type: 'warning', confirmButtonText: '删除' })
    await api<void>(`/storage-profiles/${profile.id}`, { method: 'DELETE' })
    await load()
    ElMessage.success('配置已删除')
  } catch (error) {
    if (error !== 'cancel' && error !== 'close') ElMessage.error(error instanceof Error ? error.message : '删除失败')
  }
}

function openMigration() {
  const source = profiles.value.find((profile) => !profile.is_active) ?? profiles.value[0]
  const destination = profiles.value.find((profile) => profile.id !== source?.id && !(profile.backend === 'local' && source?.backend === 'local'))
  migrationForm.value = { source_profile_id: source?.id ?? '', destination_profile_id: destination?.id ?? '' }
  migrationDialog.value = true
}
async function createMigration() {
  if (!migrationAllowed.value) return ElMessage.warning('请选择支持的迁移方向（不支持本地存储到本地存储）')
  try {
    await ElMessageBox.confirm(`将执行 ${migrationDirection.value}，目标配置会立即成为活动上传目标；源文件不会删除。`, '开始存储迁移', { type: 'warning', confirmButtonText: '开始迁移' })
  } catch { return }
  migrationSaving.value = true
  try {
    const migration = await api<StorageMigration>('/storage-migrations', { method: 'POST', body: JSON.stringify(migrationForm.value) })
    migrationDialog.value = false
    await load()
    await showMigration(migration)
    ElMessage.success('迁移任务已创建')
  } catch (error) {
    ElMessage.error(error instanceof Error ? error.message : '创建迁移失败')
  } finally { migrationSaving.value = false }
}
async function showMigration(migration: StorageMigration) {
  try {
    detail.value = await api<StorageMigrationDetail>(`/storage-migrations/${migration.id}`)
    detailDialog.value = true
    if (!isTerminal(detail.value.status)) startPolling(detail.value.id)
  } catch (error) { ElMessage.error(error instanceof Error ? error.message : '无法加载迁移详情') }
}
function startPolling(id: string) {
  stopPolling()
  pollTimer = setInterval(async () => {
    try {
      const updated = await api<StorageMigrationDetail>(`/storage-migrations/${id}`)
      detail.value = updated
      const index = migrations.value.findIndex((migration) => migration.id === id)
      if (index >= 0) migrations.value[index] = updated
      else migrations.value.unshift(updated)
      if (isTerminal(updated.status)) { stopPolling(); await loadProfiles() }
    } catch { /* a later poll or manual refresh can recover a transient failure */ }
  }, 2000)
}
function stopPolling() { if (pollTimer) { clearInterval(pollTimer); pollTimer = undefined } }
async function cancelMigration(migration: StorageMigration) {
  try {
    await ElMessageBox.confirm('当前对象完成后任务会停止，已完成的对象不会回滚。', '取消迁移', { type: 'warning', confirmButtonText: '取消任务' })
    const updated = await api<StorageMigration>(`/storage-migrations/${migration.id}/cancel`, { method: 'POST' })
    const index = migrations.value.findIndex((item) => item.id === updated.id)
    if (index >= 0) migrations.value[index] = updated
    detail.value = { ...updated, failed_items: detail.value?.failed_items ?? [] }
    if (!isTerminal(updated.status)) startPolling(updated.id)
  } catch (error) {
    if (error !== 'cancel' && error !== 'close') ElMessage.error(error instanceof Error ? error.message : '取消失败')
  }
}
async function retryMigration(migration: StorageMigration) {
  try {
    const updated = await api<StorageMigration>(`/storage-migrations/${migration.id}/retry`, { method: 'POST' })
    await load()
    await showMigration(updated)
    ElMessage.success('已重新排队未完成对象')
  } catch (error) { ElMessage.error(error instanceof Error ? error.message : '重试失败') }
}

onMounted(load)
onUnmounted(stopPolling)
</script>

<template>
  <section class="page">
    <div class="page-heading">
      <div><h1>存储设置</h1><p>激活的配置用于新安装包上传；历史文件始终按原配置读取。</p></div>
      <div class="button-group"><el-button :icon="Refresh" @click="load">刷新</el-button><el-button type="primary" :icon="Plus" @click="openCreate">新建存储配置</el-button></div>
    </div>
    <el-alert title="S3 Secret Key 采用 AES-256-GCM 加密保存，接口不会返回明文。迁移会保留源端文件。" type="info" :closable="false" show-icon class="page-alert" />
    <div class="table-scroll">
      <el-table v-loading="loading" :data="profiles" class="data-table">
        <el-table-column prop="name" label="名称" min-width="180" />
        <el-table-column label="后端" width="120"><template #default="scope"><el-tag>{{ scope.row.backend === 'local' ? '本地磁盘' : 'S3 兼容' }}</el-tag></template></el-table-column>
        <el-table-column label="状态" width="120"><template #default="scope"><el-tag :type="scope.row.is_active ? 'success' : 'info'">{{ scope.row.is_active ? '当前上传目标' : '历史配置' }}</el-tag></template></el-table-column>
        <el-table-column label="配置" min-width="260"><template #default="scope"><code>{{ scope.row.backend === 'local' ? scope.row.config.root : `${scope.row.config.endpoint || 'AWS S3'} / ${scope.row.config.bucket}` }}</code></template></el-table-column>
        <el-table-column label="操作" width="200"><template #default="scope"><el-button link @click="testProfile(scope.row)">测试</el-button><el-button v-if="!scope.row.is_active" link type="primary" :disabled="Boolean(activeMigration)" @click="activate(scope.row)">设为活动</el-button><el-button v-if="!scope.row.is_active" link type="danger" :disabled="Boolean(activeMigration)" @click="remove(scope.row)">删除</el-button></template></el-table-column>
      </el-table>
    </div>
    <el-empty v-if="!loading && profiles.length === 0" description="请先创建并激活一个存储配置，才能上传安装包" />

    <section class="migration-panel">
      <div class="panel-title"><div><h2>存储迁移</h2><p class="section-help">迁移数据库中已有的安装包，支持 S3 与本地存储之间迁移，以及 S3 到 S3。</p></div><el-button type="primary" :disabled="profiles.length < 2 || Boolean(activeMigration)" @click="openMigration">发起迁移</el-button></div>
      <div class="table-scroll">
        <el-table :data="migrations" class="data-table" empty-text="暂无迁移记录">
          <el-table-column label="方向" min-width="190"><template #default="scope"><span>{{ scope.row.source_profile_name }} → {{ scope.row.destination_profile_name }}</span><small class="table-subtitle">{{ backendLabel(scope.row.source_backend) }} → {{ backendLabel(scope.row.destination_backend) }}</small></template></el-table-column>
          <el-table-column label="状态" width="120"><template #default="scope"><el-tag :type="statusType(scope.row.status)">{{ statusLabel(scope.row.status) }}</el-tag></template></el-table-column>
          <el-table-column label="进度" min-width="230"><template #default="scope"><el-progress :percentage="progress(scope.row)" :status="scope.row.status === 'partial_failed' ? 'exception' : scope.row.status === 'completed' ? 'success' : undefined" /><small class="table-subtitle">{{ scope.row.completed_objects }} / {{ scope.row.total_objects }} 个对象 · {{ formatBytes(scope.row.completed_bytes) }} / {{ formatBytes(scope.row.total_bytes) }}</small></template></el-table-column>
          <el-table-column label="创建时间" width="180"><template #default="scope">{{ new Date(scope.row.created_at).toLocaleString('zh-CN') }}</template></el-table-column>
          <el-table-column label="操作" width="190"><template #default="scope"><el-button link @click="showMigration(scope.row)">详情</el-button><el-button v-if="['queued','running'].includes(scope.row.status)" link type="warning" @click="cancelMigration(scope.row)">取消</el-button><el-button v-if="['partial_failed','cancelled'].includes(scope.row.status)" link type="primary" @click="retryMigration(scope.row)">重试</el-button></template></el-table-column>
        </el-table>
      </div>
    </section>

    <el-dialog v-model="dialog" title="新建存储配置" width="560px" destroy-on-close><el-form label-position="top"><el-form-item label="配置名称" required><el-input v-model="form.name" placeholder="例如：生产本地磁盘" /></el-form-item><el-form-item label="存储后端"><el-radio-group v-model="form.backend"><el-radio-button value="local">本地磁盘</el-radio-button><el-radio-button value="s3">S3 兼容对象存储</el-radio-button></el-radio-group></el-form-item><template v-if="!isS3"><el-form-item label="容器内目录" required><el-input v-model="form.root" /><div class="form-help">Docker 部署请使用挂载的持久卷路径，例如 /var/lib/update-service。</div></el-form-item></template><template v-else><el-form-item label="Endpoint"><el-input v-model="form.endpoint" placeholder="http://minio:9000，可留空以使用 AWS S3" /></el-form-item><el-form-item label="区域" required><el-input v-model="form.region" /></el-form-item><el-form-item label="Bucket" required><el-input v-model="form.bucket" /></el-form-item><el-form-item label="前缀"><el-input v-model="form.prefix" placeholder="可选，例如 updates" /></el-form-item><el-form-item><el-checkbox v-model="form.path_style">使用 path-style 地址</el-checkbox></el-form-item><el-form-item label="Access Key" required><el-input v-model="form.access_key" /></el-form-item><el-form-item label="Secret Key" required><el-input v-model="form.secret_key" type="password" show-password /></el-form-item></template></el-form><template #footer><el-button @click="dialog = false">取消</el-button><el-button type="primary" :loading="saving" @click="create">创建配置</el-button></template></el-dialog>

    <el-dialog v-model="migrationDialog" title="发起存储迁移" width="560px" destroy-on-close><el-form label-position="top"><el-form-item label="源存储配置" required><el-select v-model="migrationForm.source_profile_id" class="full-width" placeholder="选择源配置"><el-option v-for="profile in profiles" :key="profile.id" :label="`${profile.name}（${backendLabel(profile.backend)}）`" :value="profile.id" /></el-select></el-form-item><el-form-item label="目标存储配置" required><el-select v-model="migrationForm.destination_profile_id" class="full-width" placeholder="选择目标配置"><el-option v-for="profile in profiles" :key="profile.id" :label="`${profile.name}（${backendLabel(profile.backend)}）`" :value="profile.id" /></el-select></el-form-item><el-alert v-if="migrationDirection && sourceProfile" :title="`${migrationDirection} · ${sourceProfile.artifact_count} 个对象 · ${formatBytes(sourceProfile.artifact_bytes)}`" type="info" :closable="false" /><el-alert v-if="sourceProfile && destinationProfile && !migrationAllowed" title="不支持本地存储到本地存储，请选择 S3 作为源或目标。" type="warning" :closable="false" /><p class="form-help migration-note">任务会迁移源配置下数据库中所有安装包。目标会立即成为活动上传配置，源端文件保留；任务可在后台运行并随时查看进度。</p></el-form><template #footer><el-button @click="migrationDialog = false">取消</el-button><el-button type="primary" :loading="migrationSaving" :disabled="!migrationAllowed" @click="createMigration">开始迁移</el-button></template></el-dialog>

    <el-dialog v-model="detailDialog" title="迁移详情" width="760px"><template v-if="detail"><div class="migration-detail-heading"><div><h3>{{ detail.source_profile_name }} → {{ detail.destination_profile_name }}</h3><p>{{ backendLabel(detail.source_backend) }} → {{ backendLabel(detail.destination_backend) }}</p></div><el-tag :type="statusType(detail.status)">{{ statusLabel(detail.status) }}</el-tag></div><el-progress :percentage="progress(detail)" :status="detail.status === 'partial_failed' ? 'exception' : detail.status === 'completed' ? 'success' : undefined" /><div class="migration-stats"><span>已完成 <strong>{{ detail.completed_objects }}</strong> / {{ detail.total_objects }} 个对象</span><span>已处理 <strong>{{ formatBytes(detail.completed_bytes) }}</strong> / {{ formatBytes(detail.total_bytes) }}</span><span v-if="detail.failed_objects" class="danger-text">失败 {{ detail.failed_objects }} 个</span><span v-if="detail.skipped_objects">跳过 {{ detail.skipped_objects }} 个</span></div><el-alert v-if="detail.last_error" :title="detail.last_error" type="error" :closable="false" /><div v-if="detail.failed_items.length" class="failed-items"><h3>未完成对象</h3><el-table :data="detail.failed_items" size="small"><el-table-column prop="object_key" label="对象 key" min-width="250" /><el-table-column prop="status" label="状态" width="100" /><el-table-column prop="attempts" label="尝试次数" width="100" /><el-table-column prop="last_error" label="错误" min-width="220" /></el-table></div></template><template #footer><el-button v-if="detail && ['queued','running'].includes(detail.status)" type="warning" @click="cancelMigration(detail)">取消任务</el-button><el-button v-if="detail && ['partial_failed','cancelled'].includes(detail.status)" type="primary" @click="retryMigration(detail)">重试未完成对象</el-button><el-button @click="detailDialog = false">关闭</el-button></template></el-dialog>
  </section>
</template>
