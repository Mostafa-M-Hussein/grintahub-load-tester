import { useState, useEffect, useCallback } from 'react';
import type { GlobalStatsSnapshot, BotStatus, ScheduleStatus } from '../types';
import * as api from '../services/api';

interface StatusBarProps {
  botStatus: BotStatus;
  stats: GlobalStatsSnapshot;
  scheduleStatus: ScheduleStatus;
  startTime: number | null;
}

interface ProxyUsage {
  usedGb: number;
  limitGb: number | null;
  remainingGb: number | null;
  loading: boolean;
  error: string | null;
}

function formatElapsedTime(startTime: number | null): string {
  if (!startTime) return '00:00:00';

  const elapsed = Math.floor((Date.now() - startTime) / 1000);
  const hours = Math.floor(elapsed / 3600);
  const minutes = Math.floor((elapsed % 3600) / 60);
  const seconds = elapsed % 60;

  return `${hours.toString().padStart(2, '0')}:${minutes.toString().padStart(2, '0')}:${seconds.toString().padStart(2, '0')}`;
}

function formatScheduleStatus(status: ScheduleStatus): string {
  if (typeof status === 'string') {
    switch (status) {
      case 'Disabled': return 'Schedule: Off';
      case 'OutsideSchedule': return 'Outside schedule';
      default: return String(status);
    }
  }
  if ('WaitingForStart' in status) {
    const mins = Math.floor(status.WaitingForStart.secondsUntil / 60);
    return `Starts in ${mins}m`;
  }
  if ('Running' in status) {
    return 'Scheduled: Running';
  }
  return 'Schedule: Unknown';
}

export function StatusBar({ botStatus, stats, scheduleStatus, startTime }: StatusBarProps) {
  const [elapsedTime, setElapsedTime] = useState('00:00:00');
  const [proxyUsage, setProxyUsage] = useState<ProxyUsage>({
    usedGb: 0,
    limitGb: null,
    remainingGb: null,
    loading: false,
    error: null,
  });

  // Update elapsed time every second ONLY when running
  useEffect(() => {
    if (!startTime) {
      setElapsedTime('00:00:00');
      return;
    }

    // Update immediately
    setElapsedTime(formatElapsedTime(startTime));

    // Only keep updating if bot is running
    if (!botStatus.isRunning) {
      // Bot stopped - freeze the timer at current value
      return;
    }

    // Then update every second while running
    const interval = setInterval(() => {
      setElapsedTime(formatElapsedTime(startTime));
    }, 1000);

    return () => clearInterval(interval);
  }, [startTime, botStatus.isRunning]);

  // Fetch Oxylabs usage
  const fetchProxyUsage = useCallback(async () => {
    setProxyUsage(prev => ({ ...prev, loading: true, error: null }));
    try {
      const usage = await api.getOxylabsUsage();
      setProxyUsage({
        usedGb: usage.trafficUsedGb,
        limitGb: usage.trafficLimitGb,
        remainingGb: usage.trafficRemainingGb,
        loading: false,
        error: usage.error,
      });
    } catch (err) {
      setProxyUsage(prev => ({
        ...prev,
        loading: false,
        error: String(err),
      }));
    }
  }, []);

  // Fetch proxy usage on mount and every 5 minutes
  useEffect(() => {
    fetchProxyUsage();
    const interval = setInterval(fetchProxyUsage, 5 * 60 * 1000);
    return () => clearInterval(interval);
  }, [fetchProxyUsage]);

  return (
    <div className={`status-bar ${botStatus.isRunning ? 'running' : 'stopped'}`}>
      <div className="status-indicator">
        <span className={`dot ${botStatus.isRunning ? 'running' : 'stopped'}`} />
        <span className="status-text">
          {botStatus.isRunning ? 'Running' : 'Stopped'}
        </span>
        <span className="elapsed-time">{elapsedTime}</span>
      </div>

      <div className="status-stats">
        <div className="stat">
          <span className="label">Sessions</span>
          <span className="value">{botStatus.activeSessions}</span>
        </div>
        <div className="stat">
          <span className="label">Ad Clicks</span>
          <span className="value">{stats.totalSuccess}</span>
        </div>
        <div className="stat">
          <span className="label">Clicks/hr</span>
          <span className="value">{stats.clicksPerHour.toFixed(1)}</span>
        </div>
        <div className="stat">
          <span className="label">Success Rate</span>
          <span className="value">
            {stats.totalClicks > 0
              ? `${((stats.totalSuccess / stats.totalClicks) * 100).toFixed(1)}%`
              : '--'}
          </span>
        </div>
        <div className="stat">
          <span className="label">Errors</span>
          <span className="value">{stats.totalErrors}</span>
        </div>
        <div className="stat">
          <span className="label">IP Changes</span>
          <span className="value">{stats.totalIpChanges || 0}</span>
        </div>
      </div>

      <div className="proxy-usage" onClick={fetchProxyUsage} title="Click to refresh Oxylabs usage">
        {proxyUsage.loading ? (
          <span className="loading">Proxy: ...</span>
        ) : proxyUsage.error ? (
          <span className="error">Proxy: --</span>
        ) : (
          <span className="usage">
            Proxy: {proxyUsage.usedGb.toFixed(2)} GB used
            {proxyUsage.remainingGb !== null && (
              <span className="remaining"> ({proxyUsage.remainingGb.toFixed(2)} GB left)</span>
            )}
          </span>
        )}
      </div>

      <div className="schedule-status">
        {formatScheduleStatus(scheduleStatus)}
      </div>
    </div>
  );
}
