import { createRouter, createWebHistory } from 'vue-router'
import { useAuthStore } from '@/stores/auth'
import AppDetailView from '@/views/AppDetailView.vue'
import AppsView from '@/views/AppsView.vue'
import LoginView from '@/views/LoginView.vue'
import MainLayout from '@/layouts/MainLayout.vue'
import StorageView from '@/views/StorageView.vue'
import UsersView from '@/views/UsersView.vue'
import AccountView from '@/views/AccountView.vue'

const router = createRouter({
  history: createWebHistory(),
  routes: [
    { path: '/login', component: LoginView, meta: { public: true } },
    { path: '/', component: MainLayout, children: [
      { path: '', redirect: '/apps' },
      { path: 'apps', component: AppsView },
      { path: 'apps/:id', component: AppDetailView, props: true },
      { path: 'users', component: UsersView, meta: { admin: true } },
      { path: 'storage', component: StorageView, meta: { admin: true } },
      { path: 'account', component: AccountView }
    ] },
    { path: '/:pathMatch(.*)*', redirect: '/apps' }
  ]
})

let initialized = false
router.beforeEach(async (to) => {
  const auth = useAuthStore()
  if (!initialized) { initialized = true; await auth.restore() }
  if (to.meta.public) return auth.user ? '/apps' : true
  if (!auth.user) return '/login'
  if (to.meta.admin && !auth.isAdmin) return '/apps'
  return true
})

export default router
