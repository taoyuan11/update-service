<script setup lang="ts">
import { ref } from 'vue'
import { useRouter } from 'vue-router'
import { ElMessage } from 'element-plus'
import { useAuthStore } from '@/stores/auth'

const username = ref(''); const password = ref(''); const busy = ref(false); const router = useRouter(); const auth = useAuthStore()
async function submit() { busy.value = true; try { await auth.login(username.value, password.value); await router.push('/apps') } catch (error) { ElMessage.error(error instanceof Error ? error.message : '无法登录') } finally { busy.value = false } }
</script>

<template>
  <main class="login-page"><section class="login-panel"><div class="login-logo">U</div><h1>更新服务</h1><p>应用发布与版本分发控制台</p><el-form label-position="top" @submit.prevent="submit"><el-form-item label="用户名"><el-input v-model="username" autocomplete="username" /></el-form-item><el-form-item label="密码"><el-input v-model="password" type="password" show-password autocomplete="current-password" @keyup.enter="submit" /></el-form-item><el-button class="login-action" type="primary" native-type="submit" :loading="busy">登录</el-button></el-form></section></main>
</template>

