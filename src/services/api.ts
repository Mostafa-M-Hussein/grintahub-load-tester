// Tauri API service
import { invoke } from '@tauri-apps/api/core';
import type {
  AppConfig,
  SessionInfo,
  GlobalStatsSnapshot,
  BotStatus,
  ScheduleConfig,
  ScheduleStatus,
} from '../types';

// Configuration
export async function configure(config: AppConfig): Promise<void> {
  return invoke('configure', { config });
}

export async function getConfig(): Promise<AppConfig> {
  return invoke('get_config');
}

// Sessions
export async function startSessions(count: number): Promise<string[]> {
  return invoke('start_sessions', { count });
}

export async function stopSessions(): Promise<void> {
  return invoke('stop_sessions');
}

export async function getSessionInfo(): Promise<SessionInfo[]> {
  return invoke('get_session_info');
}

// Statistics
export async function getGlobalStats(): Promise<GlobalStatsSnapshot> {
  return invoke('get_global_stats');
}

// Bot control
export async function startBot(): Promise<void> {
  return invoke('start_bot');
}

export async function stopBot(): Promise<void> {
  return invoke('stop_bot');
}

export async function getBotStatus(): Promise<BotStatus> {
  return invoke('get_bot_status');
}

// IP detection
export async function detectIps(): Promise<Record<string, { ok?: string; err?: string }>> {
  return invoke('detect_ips');
}

// Scheduling
export async function setSchedule(config: ScheduleConfig): Promise<void> {
  return invoke('set_schedule', { config });
}

export async function getScheduleStatus(): Promise<ScheduleStatus> {
  return invoke('get_schedule_status');
}

// Proxy testing
export interface ProxyTestResult {
  working: boolean;
  originalIp: string;
  proxyIp: string | null;
  error: string | null;
  testTimeMs: number;
}

export async function testProxy(): Promise<ProxyTestResult> {
  return invoke('test_proxy');
}

export async function isProxyVerified(): Promise<boolean> {
  return invoke('is_proxy_verified');
}

// Manual test browser (stays open for user testing)
export async function openTestBrowser(): Promise<string> {
  return invoke('open_test_browser');
}

export async function closeTestBrowser(sessionId: string): Promise<void> {
  return invoke('close_test_browser', { sessionId });
}

// Close any session by ID (user can close individual sessions)
export async function closeSession(sessionId: string): Promise<void> {
  return invoke('close_session', { sessionId });
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
  return invoke('get_captcha_balance');
}

export async function testCaptcha(): Promise<CaptchaTestResult> {
  return invoke('test_captcha');
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
  phone?: string
): Promise<AccountInfo> {
  return invoke('register_account', { name, email, phone, password });
}

export async function loginAccount(email: string, password: string): Promise<AccountInfo> {
  return invoke('login_account', { email, password });
}

export async function batchRegisterAccounts(count: number, password: string): Promise<AccountInfo[]> {
  return invoke('batch_register_accounts', { count, password });
}

export async function getSavedAccounts(): Promise<AccountInfo[]> {
  return invoke('get_saved_accounts');
}

export async function deleteAccount(email: string): Promise<void> {
  return invoke('delete_account', { email });
}
