// Dual-mode API service (Tauri + HTTP)
//
// Automatically detects whether running inside Tauri (desktop) or browser (server mode).
// In Tauri mode: uses invoke() for IPC
// In browser mode: uses fetch() against /api/ endpoints

import type {
  AppConfig,
  SessionInfo,
  GlobalStatsSnapshot,
  BotStatus,
  ScheduleConfig,
  ScheduleStatus,
} from '../types';

// Detect Tauri environment
const IS_TAURI = typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;

// Lazy-load Tauri invoke to avoid import errors in browser mode
let tauriInvoke: ((cmd: string, args?: Record<string, unknown>) => Promise<unknown>) | null = null;

async function getInvoke() {
  if (tauriInvoke) return tauriInvoke;
  const { invoke } = await import('@tauri-apps/api/core');
  tauriInvoke = invoke;
  return invoke;
}

// Get stored auth credentials for HTTP mode
function getAuthHeader(): Record<string, string> {
  const creds = localStorage.getItem('grintahub_auth');
  if (creds) {
    return { 'Authorization': `Basic ${btoa(creds)}` };
  }
  return {};
}

// HTTP fetch helper for server mode
async function apiFetch<T>(
  endpoint: string,
  options?: { method?: string; body?: unknown },
): Promise<T> {
  const method = options?.method ?? 'GET';
  const headers: Record<string, string> = {
    ...getAuthHeader(),
  };

  if (options?.body !== undefined) {
    headers['Content-Type'] = 'application/json';
  }

  const response = await fetch(`/api/${endpoint}`, {
    method,
    headers,
    body: options?.body !== undefined ? JSON.stringify(options.body) : undefined,
  });

  if (response.status === 401) {
    // Clear stored credentials and prompt for new ones
    localStorage.removeItem('grintahub_auth');
    throw new Error('Authentication required. Please refresh and enter credentials.');
  }

  if (!response.ok) {
    let errorMsg: string;
    try {
      const errorData = await response.json();
      errorMsg = errorData.error || response.statusText;
    } catch {
      errorMsg = await response.text() || response.statusText;
    }
    throw new Error(errorMsg);
  }

  // Handle empty responses (204, or empty body with 200)
  const text = await response.text();
  if (!text) return undefined as unknown as T;
  return JSON.parse(text);
}

// ========== Configuration ==========

export async function configure(config: AppConfig): Promise<void> {
  if (IS_TAURI) {
    const invoke = await getInvoke();
    return invoke('configure', { config }) as Promise<void>;
  }
  return apiFetch('config', { method: 'POST', body: config });
}

export async function getConfig(): Promise<AppConfig> {
  if (IS_TAURI) {
    const invoke = await getInvoke();
    return invoke('get_config') as Promise<AppConfig>;
  }
  return apiFetch<AppConfig>('config');
}

// ========== Sessions ==========

export async function startSessions(count: number): Promise<string[]> {
  if (IS_TAURI) {
    const invoke = await getInvoke();
    return invoke('start_sessions', { count }) as Promise<string[]>;
  }
  // Not a primary server-mode endpoint, but available
  return apiFetch<string[]>('sessions/start', { method: 'POST', body: { count } });
}

export async function stopSessions(): Promise<void> {
  if (IS_TAURI) {
    const invoke = await getInvoke();
    return invoke('stop_sessions') as Promise<void>;
  }
  return apiFetch('sessions/stop', { method: 'POST' });
}

export async function getSessionInfo(): Promise<SessionInfo[]> {
  if (IS_TAURI) {
    const invoke = await getInvoke();
    return invoke('get_session_info') as Promise<SessionInfo[]>;
  }
  return apiFetch<SessionInfo[]>('sessions');
}

// ========== Statistics ==========

export async function getGlobalStats(): Promise<GlobalStatsSnapshot> {
  if (IS_TAURI) {
    const invoke = await getInvoke();
    return invoke('get_global_stats') as Promise<GlobalStatsSnapshot>;
  }
  return apiFetch<GlobalStatsSnapshot>('stats');
}

// ========== Bot Control ==========

export async function startBot(): Promise<void> {
  if (IS_TAURI) {
    const invoke = await getInvoke();
    return invoke('start_bot') as Promise<void>;
  }
  return apiFetch('bot/start', { method: 'POST' });
}

export async function stopBot(): Promise<void> {
  if (IS_TAURI) {
    const invoke = await getInvoke();
    return invoke('stop_bot') as Promise<void>;
  }
  return apiFetch('bot/stop', { method: 'POST' });
}

export async function getBotStatus(): Promise<BotStatus> {
  if (IS_TAURI) {
    const invoke = await getInvoke();
    return invoke('get_bot_status') as Promise<BotStatus>;
  }
  return apiFetch<BotStatus>('bot/status');
}

// ========== IP Detection ==========

export async function detectIps(): Promise<Record<string, { ok?: string; err?: string }>> {
  if (IS_TAURI) {
    const invoke = await getInvoke();
    return invoke('detect_ips') as Promise<Record<string, { ok?: string; err?: string }>>;
  }
  return apiFetch('ips/detect', { method: 'POST' });
}

// ========== Scheduling ==========

export async function setSchedule(config: ScheduleConfig): Promise<void> {
  if (IS_TAURI) {
    const invoke = await getInvoke();
    return invoke('set_schedule', { config }) as Promise<void>;
  }
  return apiFetch('schedule', { method: 'POST', body: config });
}

export async function getScheduleStatus(): Promise<ScheduleStatus> {
  if (IS_TAURI) {
    const invoke = await getInvoke();
    return invoke('get_schedule_status') as Promise<ScheduleStatus>;
  }
  return apiFetch<ScheduleStatus>('schedule/status');
}

// ========== Proxy Testing ==========

export interface ProxyTestResult {
  working: boolean;
  originalIp: string;
  proxyIp: string | null;
  error: string | null;
  testTimeMs: number;
}

export async function testProxy(): Promise<ProxyTestResult> {
  if (IS_TAURI) {
    const invoke = await getInvoke();
    return invoke('test_proxy') as Promise<ProxyTestResult>;
  }
  return apiFetch<ProxyTestResult>('proxy/test', { method: 'POST' });
}

export async function isProxyVerified(): Promise<boolean> {
  if (IS_TAURI) {
    const invoke = await getInvoke();
    return invoke('is_proxy_verified') as Promise<boolean>;
  }
  return apiFetch<boolean>('proxy/verified');
}

// ========== Logs ==========

export async function getLogDir(): Promise<string> {
  if (IS_TAURI) {
    const invoke = await getInvoke();
    return invoke('get_log_dir') as Promise<string>;
  }
  const result = await apiFetch<{ path: string }>('logs/dir');
  return result.path;
}

// ========== Manual Test Browser ==========

export async function openTestBrowser(): Promise<string> {
  if (IS_TAURI) {
    const invoke = await getInvoke();
    return invoke('open_test_browser') as Promise<string>;
  }
  const result = await apiFetch<{ sessionId: string }>('browser/open', { method: 'POST' });
  return result.sessionId;
}

export async function closeTestBrowser(sessionId: string): Promise<void> {
  if (IS_TAURI) {
    const invoke = await getInvoke();
    return invoke('close_test_browser', { sessionId }) as Promise<void>;
  }
  return apiFetch('browser/close', { method: 'POST', body: { sessionId } });
}

// ========== Session Management ==========

export async function closeSession(sessionId: string): Promise<void> {
  if (IS_TAURI) {
    const invoke = await getInvoke();
    return invoke('close_session', { sessionId }) as Promise<void>;
  }
  return apiFetch('sessions/close', { method: 'POST', body: { sessionId } });
}

// ========== CAPTCHA API ==========

export interface CaptchaBalanceResult {
  balance: number;
  configured: boolean;
}

export interface CaptchaTestResult {
  success: boolean;
  solveTimeMs: number;
  totalTimeMs: number;
  tokenPreview: string;
  error: string | null;
}

export async function getCaptchaBalance(): Promise<CaptchaBalanceResult> {
  if (IS_TAURI) {
    const invoke = await getInvoke();
    return invoke('get_captcha_balance') as Promise<CaptchaBalanceResult>;
  }
  return apiFetch<CaptchaBalanceResult>('captcha/balance');
}

export async function testCaptcha(): Promise<CaptchaTestResult> {
  if (IS_TAURI) {
    const invoke = await getInvoke();
    return invoke('test_captcha') as Promise<CaptchaTestResult>;
  }
  return apiFetch<CaptchaTestResult>('captcha/test', { method: 'POST' });
}

// ========== Auth API ==========

export interface AccountInfo {
  email: string;
  name: string;
  phone: string | null;
}

export async function registerAccount(
  password: string,
  name?: string,
  email?: string,
  phone?: string,
): Promise<AccountInfo> {
  if (IS_TAURI) {
    const invoke = await getInvoke();
    return invoke('register_account', { name, email, phone, password }) as Promise<AccountInfo>;
  }
  return apiFetch<AccountInfo>('accounts/register', {
    method: 'POST',
    body: { name, email, phone, password },
  });
}

export async function loginAccount(email: string, password: string): Promise<AccountInfo> {
  if (IS_TAURI) {
    const invoke = await getInvoke();
    return invoke('login_account', { email, password }) as Promise<AccountInfo>;
  }
  return apiFetch<AccountInfo>('accounts/login', {
    method: 'POST',
    body: { email, password },
  });
}

export async function batchRegisterAccounts(
  count: number,
  password: string,
): Promise<AccountInfo[]> {
  if (IS_TAURI) {
    const invoke = await getInvoke();
    return invoke('batch_register_accounts', { count, password }) as Promise<AccountInfo[]>;
  }
  return apiFetch<AccountInfo[]>('accounts/batch', {
    method: 'POST',
    body: { count, password },
  });
}

export async function getSavedAccounts(): Promise<AccountInfo[]> {
  if (IS_TAURI) {
    const invoke = await getInvoke();
    return invoke('get_saved_accounts') as Promise<AccountInfo[]>;
  }
  return apiFetch<AccountInfo[]>('accounts');
}

export async function deleteAccount(email: string): Promise<void> {
  if (IS_TAURI) {
    const invoke = await getInvoke();
    return invoke('delete_account', { email }) as Promise<void>;
  }
  // Use POST with body since DELETE with body is non-standard
  return apiFetch('accounts', { method: 'DELETE', body: { email } });
}

// ========== Oxylabs Usage API ==========

export interface OxylabsUsage {
  trafficUsedGb: number;
  trafficLimitGb: number | null;
  trafficRemainingGb: number | null;
  periodStart: string;
  periodEnd: string;
  error: string | null;
}

export async function getOxylabsUsage(): Promise<OxylabsUsage> {
  if (IS_TAURI) {
    const invoke = await getInvoke();
    return invoke('get_oxylabs_usage') as Promise<OxylabsUsage>;
  }
  return apiFetch<OxylabsUsage>('proxy/usage');
}

// ========== Auth Helpers for Server Mode ==========

/**
 * Store basic auth credentials for server mode.
 * Call this when the user enters their credentials.
 * Format: "username:password"
 */
export function setServerAuth(username: string, password: string): void {
  localStorage.setItem('grintahub_auth', `${username}:${password}`);
}

/**
 * Clear stored auth credentials.
 */
export function clearServerAuth(): void {
  localStorage.removeItem('grintahub_auth');
}

/**
 * Check if we're running in server (browser) mode vs Tauri desktop mode.
 */
export function isServerMode(): boolean {
  return !IS_TAURI;
}
