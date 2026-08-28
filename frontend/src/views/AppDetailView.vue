<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { ElMessage, ElMessageBox } from 'element-plus'
import { ArrowLeft, Close, Document, Plus, Upload, UploadFilled } from '@element-plus/icons-vue'
import { api } from '@/api/client'
import type { App, Artifact, Page, Release, ReleaseDetail } from '@/types'

const route = useRoute(); const router = useRouter(); const appId = computed(() => route.params.id as string)
const app = ref<App>(); const releases = ref<Release[]>([]); const detail = ref<ReleaseDetail>(); const loading = ref(false); const releaseDialog = ref(false); const editDialog = ref(false); const uploadDialog = ref(false); const busy = ref(false); const selectedFile = ref<File>(); const fileInput = ref<HTMLInputElement>(); const isDragging = ref(false)
const releaseForm = ref({ version: '', channel: 'stable', release_notes: '' }); const appForm = ref({ name: '', description: '' }); const platform = ref('')
async function load() { loading.value = true; try { app.value = await api<App>(`/apps/${appId.value}`); const response = await api<Page<Release>>(`/apps/${appId.value}/releases`); releases.value = response.items } catch (error) { ElMessage.error(error instanceof Error ? error.message : '加载失败'); router.push('/apps') } finally { loading.value = false } }
async function showRelease(release: Release) { try { detail.value = await api<ReleaseDetail>(`/releases/${release.id}`) } catch (error) { ElMessage.error(error instanceof Error ? error.message : '无法加载版本') } }
function openRelease() { releaseForm.value = { version: '', channel: 'stable', release_notes: '' }; releaseDialog.value = true }
async function createRelease() { busy.value = true; try { const release = await api<Release>(`/apps/${appId.value}/releases`, { method: 'POST', body: JSON.stringify(releaseForm.value) }); releaseDialog.value = false; await load(); await showRelease(release); ElMessage.success('草稿已创建') } catch (error) { ElMessage.error(error instanceof Error ? error.message : '创建失败') } finally { busy.value = false } }
function openEdit() { if (!app.value) return; appForm.value = { name: app.value.name, description: app.value.description }; editDialog.value = true }
async function saveApp() { busy.value = true; try { app.value = await api<App>(`/apps/${appId.value}`, { method: 'PATCH', body: JSON.stringify(appForm.value) }); editDialog.value = false; ElMessage.success('应用信息已保存') } catch (error) { ElMessage.error(error instanceof Error ? error.message : '保存失败') } finally { busy.value = false } }
function chooseFile(event: Event) { selectedFile.value = (event.target as HTMLInputElement).files?.[0]; isDragging.value = false }
function openFilePicker() { fileInput.value?.click() }
function handleDragLeave(event: DragEvent) {
  const current = event.currentTarget
  const related = event.relatedTarget
  if (current instanceof Node && related instanceof Node && current.contains(related)) return
  isDragging.value = false
}
function handleDrop(event: DragEvent) { selectedFile.value = event.dataTransfer?.files?.[0]; isDragging.value = false }
function clearSelectedFile() { selectedFile.value = undefined; if (fileInput.value) fileInput.value.value = '' }
function formatBytes(bytes: number) {
  if (!Number.isFinite(bytes) || bytes < 0) return '大小未知'
  if (bytes < 1024) return `${bytes} B`
  const units = ['KB', 'MB', 'GB']
  let value = bytes
  let unit = units[0]
  for (const nextUnit of units) {
    value /= 1024
    unit = nextUnit
    if (value < 1024 || nextUnit === units[units.length - 1]) break
  }
  return `${value.toFixed(value >= 10 ? 0 : 2)} ${unit}`
}
async function uploadArtifact() { if (!detail.value || !selectedFile.value) return ElMessage.warning('请选择安装包'); busy.value = true; try { const body = new FormData(); body.append('platform', platform.value); body.append('file', selectedFile.value); await api<Artifact>(`/releases/${detail.value.id}/artifacts`, { method: 'POST', body }); uploadDialog.value = false; platform.value = ''; clearSelectedFile(); await showRelease(detail.value); ElMessage.success('安装包上传完成') } catch (error) { ElMessage.error(error instanceof Error ? error.message : '上传失败') } finally { busy.value = false } }
async function publish() { if (!detail.value) return; try { await ElMessageBox.confirm('发布后版本号、渠道和安装包都不能再修改。', '发布版本', { type: 'warning', confirmButtonText: '发布' }); const updated = await api<Release>(`/releases/${detail.value.id}/publish`, { method: 'POST' }); await load(); await showRelease(updated); ElMessage.success('版本已发布') } catch (error) { if (error !== 'cancel' && error !== 'close') ElMessage.error(error instanceof Error ? error.message : '发布失败') } }
async function withdraw() { if (!detail.value) return; try { await ElMessageBox.confirm('下架后客户端不再发现这个版本，已有下载链接也会失效。', '下架版本', { type: 'warning', confirmButtonText: '下架' }); const updated = await api<Release>(`/releases/${detail.value.id}/withdraw`, { method: 'POST' }); await load(); await showRelease(updated); ElMessage.success('版本已下架') } catch (error) { if (error !== 'cancel' && error !== 'close') ElMessage.error(error instanceof Error ? error.message : '下架失败') } }
async function deleteArtifact(artifact: Artifact) { if (!detail.value) return; try { await ElMessageBox.confirm(`确认删除 ${artifact.original_file_name}？`, '删除安装包', { type: 'warning' }); await api<void>(`/artifacts/${artifact.id}`, { method: 'DELETE' }); await showRelease(detail.value); ElMessage.success('安装包已删除') } catch (error) { if (error !== 'cancel' && error !== 'close') ElMessage.error(error instanceof Error ? error.message : '删除失败') } }
onMounted(load)
</script>

<template>
  <section class="page" v-loading="loading">
    <div class="crumb-action"><el-tooltip content="返回应用" placement="bottom"><el-button text :icon="ArrowLeft" aria-label="返回应用" @click="router.push('/apps')" /></el-tooltip></div>
    <div v-if="app" class="app-header"><div><div class="heading-row"><h1>{{ app.name }}</h1><el-tag :type="app.status === 'active' ? 'success' : 'info'">{{ app.status === 'active' ? '正常' : '已删除' }}</el-tag></div><p>{{ app.description || '暂无应用说明' }}</p><code>{{ app.id }}</code></div><el-button @click="openEdit">编辑信息</el-button></div>
    <div class="detail-layout">
      <section class="release-list"><div class="panel-title"><h2>版本发布</h2><el-button type="primary" size="small" :icon="Plus" :disabled="app?.status !== 'active'" @click="openRelease">新建草稿</el-button></div><div v-if="releases.length" class="release-menu"><button v-for="release in releases" :key="release.id" class="release-row" :class="{ selected: detail?.id === release.id }" @click="showRelease(release)"><span><strong>{{ release.version }}</strong><small>{{ release.channel === 'stable' ? '稳定渠道' : '测试渠道' }}</small></span><el-tag size="small" :type="release.status === 'published' ? 'success' : release.status === 'draft' ? 'warning' : 'info'">{{ release.status === 'published' ? '已发布' : release.status === 'draft' ? '草稿' : '已下架' }}</el-tag></button></div><el-empty v-else description="尚未创建版本" :image-size="72" /></section>
      <section class="release-workspace">
        <el-empty v-if="!detail" description="从左侧选择一个版本" />
        <template v-else>
          <div class="release-heading">
            <div>
              <div class="heading-row"><h2>{{ detail.version }}</h2><el-tag>{{ detail.channel }}</el-tag><el-tag :type="detail.status === 'published' ? 'success' : detail.status === 'draft' ? 'warning' : 'info'">{{ detail.status === 'published' ? '已发布' : detail.status === 'draft' ? '草稿' : '已下架' }}</el-tag></div>
              <p>{{ detail.release_notes || '暂无更新日志' }}</p>
            </div>
            <div class="button-group"><el-button v-if="detail.status === 'draft'" type="primary" @click="publish">发布</el-button><el-button v-if="detail.status === 'published'" type="warning" @click="withdraw">下架</el-button></div>
          </div>
          <div class="panel-title"><h3>安装包</h3><el-button v-if="detail.status === 'draft'" size="small" :icon="Upload" @click="uploadDialog = true">上传安装包</el-button></div>
          <div class="table-scroll">
            <el-table :data="detail.artifacts" class="data-table" size="small" empty-text="尚未上传安装包">
              <el-table-column prop="platform" label="平台" width="150" />
              <el-table-column prop="original_file_name" label="文件" min-width="200" />
              <el-table-column label="大小" width="120"><template #default="scope">{{ (scope.row.size_bytes / 1024 / 1024).toFixed(2) }} MB</template></el-table-column>
              <el-table-column prop="sha256" label="SHA-256" min-width="180"><template #default="scope"><code class="hash">{{ scope.row.sha256 }}</code></template></el-table-column>
              <el-table-column v-if="detail.status === 'draft'" label="操作" width="80"><template #default="scope"><el-button link type="danger" @click="deleteArtifact(scope.row)">删除</el-button></template></el-table-column>
            </el-table>
          </div>
        </template>
      </section>
    </div>
    <el-dialog v-model="releaseDialog" title="新建版本草稿" width="520px"><el-form label-position="top"><el-form-item label="版本号" required><el-input v-model="releaseForm.version" placeholder="例如 1.2.0" /></el-form-item><el-form-item label="渠道"><el-radio-group v-model="releaseForm.channel"><el-radio-button value="stable">稳定版</el-radio-button><el-radio-button value="beta">测试版</el-radio-button></el-radio-group></el-form-item><el-form-item label="更新日志"><el-input v-model="releaseForm.release_notes" type="textarea" :rows="5" /></el-form-item></el-form><template #footer><el-button @click="releaseDialog = false">取消</el-button><el-button type="primary" :loading="busy" @click="createRelease">创建草稿</el-button></template></el-dialog>
    <el-dialog v-model="editDialog" title="编辑应用" width="480px"><el-form label-position="top"><el-form-item label="应用名称"><el-input v-model="appForm.name" /></el-form-item><el-form-item label="应用说明"><el-input v-model="appForm.description" type="textarea" :rows="4" /></el-form-item></el-form><template #footer><el-button @click="editDialog = false">取消</el-button><el-button type="primary" :loading="busy" @click="saveApp">保存</el-button></template></el-dialog>
    <el-dialog v-model="uploadDialog" title="上传安装包" width="480px"><el-form label-position="top"><el-form-item label="平台标识" required><el-input v-model="platform" placeholder="例如 windows-x64 或 android-arm64" /></el-form-item><el-form-item label="安装包" required><div class="file-dropzone" :class="{ 'is-dragging': isDragging, 'has-file': selectedFile }" @click="openFilePicker" @dragenter.prevent="isDragging = true" @dragover.prevent="isDragging = true" @dragleave.prevent="handleDragLeave" @drop.prevent="handleDrop"><input ref="fileInput" class="file-input-hidden" type="file" tabindex="-1" aria-hidden="true" @click.stop @change="chooseFile" /><button type="button" class="file-picker-surface" aria-label="选择安装包文件" @click.stop="openFilePicker"><template v-if="selectedFile"><span class="file-state-icon"><el-icon><Document /></el-icon></span><span class="file-state-copy"><strong :title="selectedFile.name">{{ selectedFile.name }}</strong><small>{{ formatBytes(selectedFile.size) }} · 点击更换文件</small></span></template><template v-else><span class="file-state-icon"><el-icon><UploadFilled /></el-icon></span><span class="file-empty-copy"><strong>点击选择文件</strong><small>也可以将安装包拖拽到这里</small></span></template></button><el-button v-if="selectedFile" class="file-remove" text circle :icon="Close" aria-label="移除已选文件" @click.stop="clearSelectedFile" /></div></el-form-item><p class="form-help">每个版本中的平台标识必须唯一。上传时系统会计算 SHA-256。</p></el-form><template #footer><el-button @click="uploadDialog = false">取消</el-button><el-button type="primary" :loading="busy" @click="uploadArtifact">上传</el-button></template></el-dialog>
  </section>
</template>
