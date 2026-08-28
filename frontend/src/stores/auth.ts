import { computed, ref } from 'vue'
import { defineStore } from 'pinia'
import { api, setCsrfToken } from '@/api/client'
import type { AuthResponse, User } from '@/types'

export const useAuthStore = defineStore('auth', () => {
  const user = ref<User | null>(null)
  const loading = ref(false)
  const isAdmin = computed(() => user.value?.role === 'admin')

  function apply(response: AuthResponse) { user.value = response.user; setCsrfToken(response.csrf_token) }
  async function restore() { try { apply(await api<AuthResponse>('/auth/me')) } catch { user.value = null; setCsrfToken('') } }
  async function login(username: string, password: string) { apply(await api<AuthResponse>('/auth/login', { method: 'POST', body: JSON.stringify({ username, password }) })) }
  async function logout() { await api<void>('/auth/logout', { method: 'POST' }); user.value = null; setCsrfToken('') }
  return { user, loading, isAdmin, apply, restore, login, logout }
})

