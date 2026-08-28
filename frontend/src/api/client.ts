let csrfToken = ''
export const setCsrfToken = (value: string) => { csrfToken = value }

export class ApiError extends Error { constructor(public status: number, message: string) { super(message) } }

export async function api<T>(path: string, options: RequestInit = {}): Promise<T> {
  const headers = new Headers(options.headers)
  if (options.body && !(options.body instanceof FormData)) headers.set('Content-Type', 'application/json')
  if (!['GET', 'HEAD'].includes(options.method ?? 'GET') && csrfToken) headers.set('X-CSRF-Token', csrfToken)
  const response = await fetch(`/api${path}`, { ...options, headers, credentials: 'include' })
  if (response.status === 204) return undefined as T
  if (!response.ok) {
    const body = await response.json().catch(() => null) as { message?: string } | null
    throw new ApiError(response.status, body?.message ?? `请求失败 (${response.status})`)
  }
  return response.json() as Promise<T>
}

