<script setup lang="ts">
import { ref } from 'vue'
import { ElMessage } from 'element-plus'
import { api } from '@/api/client'

const form = ref({ current_password: '', new_password: '', confirmation: '' })
const saving = ref(false)
async function save() {
  if (form.value.new_password !== form.value.confirmation) return ElMessage.warning('两次输入的新密码不一致')
  saving.value = true
  try {
    await api<void>('/auth/me/password', { method: 'POST', body: JSON.stringify({ current_password: form.value.current_password, new_password: form.value.new_password }) })
    form.value = { current_password: '', new_password: '', confirmation: '' }
    ElMessage.success('密码已更新，其他登录会话已失效')
  } catch (error) { ElMessage.error(error instanceof Error ? error.message : '修改失败') } finally { saving.value = false }
}
</script>

<template>
  <section class="page account-page"><div class="page-heading"><div><h1>账户设置</h1><p>修改当前账户的登录密码。</p></div></div><div class="form-panel"><h2>修改密码</h2><el-form label-position="top" @submit.prevent="save"><el-form-item label="当前密码"><el-input v-model="form.current_password" type="password" show-password autocomplete="current-password" /></el-form-item><el-form-item label="新密码"><el-input v-model="form.new_password" type="password" show-password autocomplete="new-password" /></el-form-item><el-form-item label="确认新密码"><el-input v-model="form.confirmation" type="password" show-password autocomplete="new-password" /></el-form-item><el-button type="primary" native-type="submit" :loading="saving">保存新密码</el-button></el-form></div></section>
</template>
