import type { SessionInfo } from '../types';

interface SessionDashboardProps {
  sessions: SessionInfo[];
  onDetectIps: () => void;
  onCloseSession?: (sessionId: string) => void;
}

function getStatusText(status: SessionInfo['status']): string {
  if (typeof status === 'string') {
    return status;
  }
  if ('error' in status) {
    return `Error: ${status.error}`;
  }
  return 'Unknown';
}

function getStatusClass(status: SessionInfo['status']): string {
  if (typeof status === 'string') {
    switch (status) {
      case 'Running': return 'status-running';
      case 'Starting': return 'status-starting';
      case 'Paused': return 'status-paused';
      case 'Stopped': return 'status-stopped';
      default: return '';
    }
  }
  if ('error' in status) {
    return 'status-error';
  }
  return '';
}

export function SessionDashboard({ sessions, onDetectIps, onCloseSession }: SessionDashboardProps) {
  if (sessions.length === 0) {
    return (
      <div className="session-dashboard empty">
        <p>No active sessions</p>
        <p className="hint">Start the bot to create browser sessions</p>
      </div>
    );
  }

  const totalClicks = sessions.reduce((sum, s) => sum + s.clickCount, 0);
  const totalErrors = sessions.reduce((sum, s) => sum + s.errorCount, 0);

  return (
    <div className="session-dashboard">
      <div className="dashboard-header">
        <h2>Sessions ({sessions.length})</h2>
        <div className="dashboard-summary">
          <span className="stat">
            Total Clicks: <strong>{totalClicks}</strong>
          </span>
          <span className="stat errors">
            Errors: <strong>{totalErrors}</strong>
          </span>
        </div>
        <button onClick={onDetectIps} className="detect-btn">
          Detect IPs
        </button>
      </div>

      <div className="sessions-grid">
        {sessions.map(session => (
          <div key={session.id} className={`session-card ${session.alive ? 'alive' : 'dead'}`}>
            <div className="session-header">
              <span className="session-id">{session.id}</span>
              <span className={`session-status ${getStatusClass(session.status)}`}>
                {getStatusText(session.status)}
              </span>
            </div>

            <div className="session-ip">
              {session.currentIp ? (
                <>
                  <span className="ip-label">IP:</span>
                  <span className="ip-value">{session.currentIp}</span>
                  {session.ipChangeCount > 0 && (
                    <span className="ip-change-badge" title={`Previous: ${session.previousIp || 'N/A'}`}>
                      IP Changed x{session.ipChangeCount}
                    </span>
                  )}
                </>
              ) : (
                <span className="ip-unknown">IP: Unknown</span>
              )}
            </div>

            <div className="session-stats">
              <div className="stat">
                <span className="label">Cycles</span>
                <span className="value">{session.cycleCount}</span>
              </div>
              <div className="stat">
                <span className="label">Clicks</span>
                <span className="value">{session.clickCount}</span>
              </div>
              <div className="stat errors">
                <span className="label">Errors</span>
                <span className="value">{session.errorCount}</span>
              </div>
              {session.captchaCount > 0 && (
                <div className="stat captcha">
                  <span className="label">CAPTCHAs</span>
                  <span className="value">{session.captchaCount}</span>
                </div>
              )}
            </div>

            <div className="session-footer">
              <div className={`session-alive-indicator ${session.alive ? 'alive' : 'dead'}`}>
                {session.alive ? 'Active' : 'Inactive'}
              </div>
              {session.alive && onCloseSession && (
                <button
                  className="close-session-btn"
                  onClick={() => onCloseSession(session.id)}
                  title="Close this session"
                >
                  Close
                </button>
              )}
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}
