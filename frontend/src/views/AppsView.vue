<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { ElMessage, ElMessageBox } from 'element-plus'
import { Plus, Search } from '@element-plus/icons-vue'
import { api } from '@/api/client'
import type { App, Page } from '@/types'

const apps = ref<App[]>([]); const total = ref(0); const loading = ref(false); const search = ref(''); const dialog = ref(false); const saving = ref(false)
const form = ref({ name: '', description: '' })
async function load() { loading.value = true; try { const q = search.value ? `?name=${encodeURIComponent(search.value)}` : ''; const result = await api<Page<App>>(`/apps${q}`); apps.value = result.items; total.value = result.total } catch (error) { ElMessage.error(error instanceof Error ? error.message : '加载失败') } finally { loading.value = false } }
function openCreate() { form.value = { name: '', description: '' }; dialog.value = true }
async function create() { saving.value = true; try { const app = await api<App>('/apps', { method: 'POST', body: JSON.stringify(form.value) }); dialog.value = false; ElMessage.success('应用已创建'); window.location.assign(`/apps/${app.id}`) } catch (error) { ElMessage.error(error instanceof Error ? error.message : '创建失败') } finally { saving.value = false } }
async function remove(app: App) { try { await ElMessageBox.confirm(`“${app.name}” 将立刻停止提供公开更新和下载。`, '删除应用', { type: 'warning', confirmButtonText: '删除' }); await api<void>(`/apps/${app.id}`, { method: 'DELETE' }); ElMessage.success('应用已删除'); await load() } catch (error) { if (error !== 'cancel' && error !== 'close') ElMessage.error(error instanceof Error ? error.message : '删除失败') } }
onMounted(load)
</script>

<template>
  <section class="page">
    <div class="page-heading"><div><h1>应用与发布</h1><p>创建应用，维护多渠道、多平台的版本发布。</p></div><el-button type="primary" :icon="Plus" @click="openCreate">新建应用</el-button></div>
    <div class="toolbar"><el-input v-model="search" placeholder="搜索应用名称" clearable @keyup.enter="load"><template #prefix><el-icon><Search /></el-icon></template></el-input><el-button @click="load">搜索</el-button><span class="result-count">共 {{ total }} 个应用</span></div>
    <div v-loading="loading" class="app-grid">
      <article v-for="app in apps" :key="app.id" class="app-item">
        <div class="app-item-top"><span class="app-icon">{{ app.name.slice(0, 1).toUpperCase() }}</span><el-tag size="small" :type="app.status === 'active' ? 'success' : 'info'">{{ app.status === 'active' ? '正常' : '已删除' }}</el-tag></div>
        <router-link class="app-link" :to="`/apps/${app.id}`">{{ app.name }}</router-link><p>{{ app.description || '暂无应用说明' }}</p><code>{{ app.id }}</code>
        <div class="app-item-footer"><span>{{ new Date(app.updated_at).toLocaleString('zh-CN') }}</span><el-button v-if="app.status === 'active'" link type="danger" @click="remove(app)">删除</el-button></div>
      </article>
      <el-empty v-if="!loading && apps.length === 0" description="暂无应用，先创建一个应用" />
    </div>
    <el-dialog v-model="dialog" title="新建应用" width="480px" destroy-on-close><el-form label-position="top" @submit.prevent="create"><el-form-item label="应用名称" required><el-input v-model="form.name" maxlength="160" show-word-limit /></el-form-item><el-form-item label="应用说明"><el-input v-model="form.description" type="textarea" :rows="4" maxlength="10000" show-word-limit /></el-form-item></el-form><template #footer><el-button @click="dialog = false">取消</el-button><el-button type="primary" :loading="saving" @click="create">创建</el-button></template></el-dialog>
  </section>
</template>

