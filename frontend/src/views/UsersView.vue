<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { ElMessage } from 'element-plus'
import { Plus } from '@element-plus/icons-vue'
import { api } from '@/api/client'
import type { Page, Role, User } from '@/types'

const users = ref<User[]>([]); const loading = ref(false); const dialog = ref(false); const resetDialog = ref(false); const saving = ref(false); const target = ref<User>(); const form = ref({ username: '', password: '', role: 'user' as Role }); const resetPassword = ref('')
async function load() { loading.value = true; try { users.value = (await api<Page<User>>('/users')).items } catch (error) { ElMessage.error(error instanceof Error ? error.message : '加载失败') } finally { loading.value = false } }
function openCreate() { form.value = { username: '', password: '', role: 'user' }; dialog.value = true }
async function create() { saving.value = true; try { await api<User>('/users', { method: 'POST', body: JSON.stringify(form.value) }); dialog.value = false; await load(); ElMessage.success('用户已创建') } catch (error) { ElMessage.error(error instanceof Error ? error.message : '创建失败') } finally { saving.value = false } }
async function toggle(user: User) { try { await api<User>(`/users/${user.id}`, { method: 'PATCH', body: JSON.stringify({ enabled: !user.enabled }) }); await load(); ElMessage.success(user.enabled ? '用户已禁用，所有会话已撤销' : '用户已启用') } catch (error) { ElMessage.error(error instanceof Error ? error.message : '操作失败') } }
async function setRole(user: User, role: Role) { try { await api<User>(`/users/${user.id}`, { method: 'PATCH', body: JSON.stringify({ role }) }); await load(); ElMessage.success('角色已更新') } catch (error) { ElMessage.error(error instanceof Error ? error.message : '更新失败') } }
function openReset(user: User) { target.value = user; resetPassword.value = ''; resetDialog.value = true }
async function reset() { if (!target.value) return; saving.value = true; try { await api<User>(`/users/${target.value.id}`, { method: 'PATCH', body: JSON.stringify({ password: resetPassword.value }) }); resetDialog.value = false; ElMessage.success('密码已重置，该用户现有会话已失效') } catch (error) { ElMessage.error(error instanceof Error ? error.message : '重置失败') } finally { saving.value = false } }
onMounted(load)
</script>

<template>
  <section class="page">
    <div class="page-heading"><div><h1>用户管理</h1><p>管理员可创建用户、分配角色、重置密码或禁用账号。</p></div><el-button type="primary" :icon="Plus" @click="openCreate">新建用户</el-button></div>
    <div class="table-scroll">
      <el-table v-loading="loading" :data="users" class="data-table">
        <el-table-column prop="username" label="用户名" min-width="180" />
        <el-table-column label="角色" width="170"><template #default="scope"><el-select :model-value="scope.row.role" size="small" @change="setRole(scope.row, $event as Role)"><el-option label="管理员" value="admin" /><el-option label="普通用户" value="user" /></el-select></template></el-table-column>
        <el-table-column label="状态" width="120"><template #default="scope"><el-tag :type="scope.row.enabled ? 'success' : 'info'">{{ scope.row.enabled ? '启用' : '已禁用' }}</el-tag></template></el-table-column>
        <el-table-column label="创建时间" min-width="180"><template #default="scope">{{ new Date(scope.row.created_at).toLocaleString('zh-CN') }}</template></el-table-column>
        <el-table-column label="操作" width="180"><template #default="scope"><el-button link @click="openReset(scope.row)">重置密码</el-button><el-button link :type="scope.row.enabled ? 'danger' : 'success'" @click="toggle(scope.row)">{{ scope.row.enabled ? '禁用' : '启用' }}</el-button></template></el-table-column>
      </el-table>
    </div>
    <el-dialog v-model="dialog" title="新建用户" width="440px"><el-form label-position="top"><el-form-item label="用户名" required><el-input v-model="form.username" /></el-form-item><el-form-item label="初始密码" required><el-input v-model="form.password" type="password" show-password /></el-form-item><el-form-item label="角色"><el-radio-group v-model="form.role"><el-radio-button value="user">普通用户</el-radio-button><el-radio-button value="admin">管理员</el-radio-button></el-radio-group></el-form-item></el-form><template #footer><el-button @click="dialog = false">取消</el-button><el-button type="primary" :loading="saving" @click="create">创建</el-button></template></el-dialog>
    <el-dialog v-model="resetDialog" title="重置密码" width="420px"><p>正在重置 {{ target?.username }} 的密码。</p><el-input v-model="resetPassword" type="password" show-password /><template #footer><el-button @click="resetDialog = false">取消</el-button><el-button type="primary" :loading="saving" @click="reset">重置密码</el-button></template></el-dialog>
  </section>
</template>
