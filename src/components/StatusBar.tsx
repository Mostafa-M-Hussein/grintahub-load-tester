import type { GlobalStatsSnapshot, BotStatus, ScheduleStatus } from '../types';

interface StatusBarProps {
  botStatus: BotStatus;
  stats: GlobalStatsSnapshot;
  scheduleStatus: ScheduleStatus;
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

export function StatusBar({ botStatus, stats, scheduleStatus }: StatusBarProps) {
  return (
    <div className={`status-bar ${botStatus.isRunning ? 'running' : 'stopped'}`}>
      <div className="status-indicator">
        <span className={`dot ${botStatus.isRunning ? 'running' : 'stopped'}`} />
        <span className="status-text">
          {botStatus.isRunning ? 'Running' : 'Stopped'}
        </span>
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
      </div>

      <div className="schedule-status">
        {formatScheduleStatus(scheduleStatus)}
      </div>
    </div>
  );
}
