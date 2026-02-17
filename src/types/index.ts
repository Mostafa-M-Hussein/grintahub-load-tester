// TypeScript types matching Rust backend

export interface AppConfig {
  proxyCustomer: string;
  proxyPassword: string;
  proxyCountry: string;
  proxySesstime?: number;  // Session time in minutes (how long to keep same IP)
  proxyVerified?: boolean;
  captchaApiKey?: string;
  concurrentSessions: number;
  headless: boolean;
  clicksPerHour: number;
  minDelayMs: number;
  maxDelayMs: number;
  maxClicksPerSession?: number;  // Max clicks before session auto-closes (0 = unlimited)
  keywords: string[];
  schedule: ScheduleConfig;
  accounts?: SavedAccount[];
  // Auto-rotate IP after all keywords are searched (change IP on keyword cycle completion)
  autoRotateIp?: boolean;
  // Pick keywords randomly instead of cycling sequentially
  randomKeywords?: boolean;
  // Target domains to click ads for (e.g., ["grintahub.com", "golden4tic.com"])
  targetDomains?: string[];
}

export interface SavedAccount {
  id?: number;
  name: string;
  email: string;
  phone?: string;
  token?: string;
  cookies?: string;
}

export interface ScheduleConfig {
  enabled: boolean;
  startTime: string;
  endTime: string;
  days: number[];
  cronExpression?: string;
}

export interface SessionInfo {
  id: string;
  alive: boolean;
  currentIp?: string;
  previousIp?: string;
  ipChangeCount: number;
  clickCount: number;
  errorCount: number;
  cycleCount: number;
  captchaCount: number;
  status: SessionStatus;
}

export type SessionStatus =
  | { type: 'starting' }
  | { type: 'running' }
  | { type: 'paused' }
  | { type: 'error'; message: string }
  | { type: 'stopped' };

export interface GlobalStatsSnapshot {
  totalClicks: number;
  totalSuccess: number;
  totalErrors: number;
  averageLatencyMs: number;
  clicksPerHour: number;
  activeSessions: number;
  totalIpChanges: number;
}

export interface BotStatus {
  isRunning: boolean;
  activeSessions: number;
  totalClicks: number;
  clicksPerHour: number;
}

// Rust enum serializes with variant name as key
export type ScheduleStatus =
  | 'Disabled'
  | 'OutsideSchedule'
  | { WaitingForStart: { secondsUntil: number } }
  | { Running: { secondsUntilEnd: number | null } };
