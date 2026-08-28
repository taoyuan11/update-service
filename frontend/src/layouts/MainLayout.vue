<script setup lang="ts">
import { computed } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { Box, Setting, SwitchButton, UserFilled } from '@element-plus/icons-vue'
import { useAuthStore } from '@/stores/auth'

const auth = useAuthStore(); const route = useRoute(); const router = useRouter()
const active = computed(() => route.path)
async function signOut() { await auth.logout(); await router.push('/login') }
</script>

<template>
  <el-container class="shell">
    <el-aside width="244px" class="sidebar">
      <div class="brand"><span class="brand-mark">U</span><span>更新服务</span></div>
      <el-menu :default-active="active" router background-color="transparent" text-color="#a9b5c7" active-text-color="#ffffff">
        <el-menu-item index="/apps"><el-icon><Box /></el-icon><span>应用与发布</span></el-menu-item>
        <el-menu-item v-if="auth.isAdmin" index="/users"><el-icon><UserFilled /></el-icon><span>用户管理</span></el-menu-item>
        <el-menu-item v-if="auth.isAdmin" index="/storage"><el-icon><Setting /></el-icon><span>存储设置</span></el-menu-item>
      </el-menu>
      <div class="sidebar-user">
        <router-link to="/account">
          <span class="user-avatar">{{ auth.user?.username?.slice(0, 1).toUpperCase() }}</span>
          <span><strong>{{ auth.user?.username }}</strong><small>{{ auth.isAdmin ? '管理员' : '普通用户' }}</small></span>
        </router-link>
        <el-tooltip content="退出登录" placement="right"><el-button text :icon="SwitchButton" aria-label="退出登录" @click="signOut" /></el-tooltip>
      </div>
    </el-aside>
    <el-main class="content"><router-view /></el-main>
  </el-container>
</template>
