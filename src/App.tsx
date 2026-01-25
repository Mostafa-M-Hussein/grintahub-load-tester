import { useState, useEffect, useCallback } from 'react';
import { ConfigPanel } from './components/ConfigPanel';
import { AccountsPanel } from './components/AccountsPanel';
import { SessionDashboard } from './components/SessionDashboard';
import { StatusBar } from './components/StatusBar';
import { Controls } from './components/Controls';
import * as api from './services/api';
import type {
  AppConfig,
  SessionInfo,
  GlobalStatsSnapshot,
  BotStatus,
  ScheduleStatus,
} from './types';
import './App.css';

const DEFAULT_CONFIG: AppConfig = {
  proxyCustomer: '',
  proxyPassword: '',
  proxyCountry: 'sa',
  concurrentSessions: 3,
  headless: false,
  clicksPerHour: 60,
  minDelayMs: 5000,
  maxDelayMs: 30000,
  keywords: [
    'تذاكر نادي الهلال',
    'تذاكر الهلال والاهلي',
    'تذاكر الهلال',
    'منصة بيع تذاكر الهلال',
    'حجز تذاكر مباراة الهلال والاهلي',
  ],
  schedule: {
    enabled: false,
    startTime: '09:00',
    endTime: '18:00',
    days: [0, 1, 2, 3, 4],
  },
};

const DEFAULT_STATS: GlobalStatsSnapshot = {
  totalClicks: 0,
  totalSuccess: 0,
  totalErrors: 0,
  averageLatencyMs: 0,
  clicksPerHour: 0,
  activeSessions: 0,
};

const DEFAULT_BOT_STATUS: BotStatus = {
  isRunning: false,
  activeSessions: 0,
  totalClicks: 0,
  clicksPerHour: 0,
};

type TabType = 'config' | 'accounts';

function App() {
  const [config, setConfig] = useState<AppConfig>(DEFAULT_CONFIG);
  const [sessions, setSessions] = useState<SessionInfo[]>([]);
  const [stats, setStats] = useState<GlobalStatsSnapshot>(DEFAULT_STATS);
  const [botStatus, setBotStatus] = useState<BotStatus>(DEFAULT_BOT_STATUS);
  const [scheduleStatus, setScheduleStatus] = useState<ScheduleStatus>('Disabled' as unknown as ScheduleStatus);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [activeTab, setActiveTab] = useState<TabType>('config');

  // Load initial config
  useEffect(() => {
    api.getConfig()
      .then(setConfig)
      .catch(err => console.error('Failed to load config:', err));
  }, []);

  // Poll for status updates
  useEffect(() => {
    const interval = setInterval(async () => {
      try {
        const [newStatus, newStats, newSessions, newSchedule] = await Promise.all([
          api.getBotStatus(),
          api.getGlobalStats(),
          api.getSessionInfo(),
          api.getScheduleStatus(),
        ]);
        setBotStatus(newStatus);
        setStats(newStats);
        setSessions(newSessions);
        setScheduleStatus(newSchedule);
      } catch (err) {
        console.error('Failed to fetch status:', err);
      }
    }, 2000);

    return () => clearInterval(interval);
  }, []);

  const handleSaveConfig = useCallback(async (newConfig: AppConfig) => {
    setLoading(true);
    setError(null);
    try {
      await api.configure(newConfig);
      setConfig(newConfig);
    } catch (err) {
      setError(String(err));
    } finally {
      setLoading(false);
    }
  }, []);

  const handleStart = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      // Auto-save config before starting to ensure latest settings are used
      await api.configure(config);
      await api.startBot();
    } catch (err) {
      setError(String(err));
    } finally {
      setLoading(false);
    }
  }, [config]);

  const handleStop = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      await api.stopBot();
    } catch (err) {
      setError(String(err));
    } finally {
      setLoading(false);
    }
  }, []);

  const handleDetectIps = useCallback(async () => {
    setLoading(true);
    try {
      await api.detectIps();
      // Refresh sessions to show detected IPs
      const newSessions = await api.getSessionInfo();
      setSessions(newSessions);
    } catch (err) {
      setError(String(err));
    } finally {
      setLoading(false);
    }
  }, []);

  const handleCloseSession = useCallback(async (sessionId: string) => {
    try {
      await api.closeSession(sessionId);
      // Refresh sessions list
      const newSessions = await api.getSessionInfo();
      setSessions(newSessions);
    } catch (err) {
      setError(String(err));
    }
  }, []);

  return (
    <div className="app">
      <header className="app-header">
        <h1>GrintaHub Clicker</h1>
        <Controls
          isRunning={botStatus.isRunning}
          onStart={handleStart}
          onStop={handleStop}
          disabled={loading}
        />
      </header>

      {error && (
        <div className="error-banner">
          <span>{error}</span>
          <button onClick={() => setError(null)}>Dismiss</button>
        </div>
      )}

      <StatusBar
        botStatus={botStatus}
        stats={stats}
        scheduleStatus={scheduleStatus}
      />

      <main className="app-main">
        <div className="left-panel">
          <div className="panel-tabs">
            <button
              className={`tab-btn ${activeTab === 'config' ? 'active' : ''}`}
              onClick={() => setActiveTab('config')}
            >
              Configuration
            </button>
            <button
              className={`tab-btn ${activeTab === 'accounts' ? 'active' : ''}`}
              onClick={() => setActiveTab('accounts')}
            >
              Accounts
            </button>
          </div>

          {activeTab === 'config' ? (
            <ConfigPanel
              config={config}
              onSave={handleSaveConfig}
              disabled={botStatus.isRunning || loading}
            />
          ) : (
            <AccountsPanel
              config={config}
              onConfigChange={handleSaveConfig}
              disabled={botStatus.isRunning || loading}
            />
          )}
        </div>

        <div className="right-panel">
          <SessionDashboard
            sessions={sessions}
            onDetectIps={handleDetectIps}
            onCloseSession={handleCloseSession}
          />
        </div>
      </main>
    </div>
  );
}

export default App;
