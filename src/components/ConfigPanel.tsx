import { useState, useEffect } from 'react';
import type { AppConfig, ScheduleConfig, SavedAccount } from '../types';
import * as api from '../services/api';

interface ConfigPanelProps {
  config: AppConfig;
  onSave: (config: AppConfig) => void;
  disabled?: boolean;
}

interface GoogleAccountState {
  email: string;
  password: string;
  enabled: boolean;
}

interface ProxyStatus {
  tested: boolean;
  working: boolean;
  originalIp: string | null;
  proxyIp: string | null;
  error: string | null;
  testTimeMs: number;
  testing: boolean;
}

const DAYS = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun'];

export function ConfigPanel({ config, onSave, disabled }: ConfigPanelProps) {
  const [localConfig, setLocalConfig] = useState<AppConfig>(config);
  const [keywordsText, setKeywordsText] = useState(config.keywords.join(', '));
  const [googleAccount, setGoogleAccount] = useState<GoogleAccountState>({
    email: config.accounts?.[0]?.email || '',
    password: config.accounts?.[0]?.token || '', // Load password from saved token
    enabled: !!config.accounts?.[0]?.email,
  });
  const [proxyStatus, setProxyStatus] = useState<ProxyStatus>({
    tested: false,
    working: false,
    originalIp: null,
    proxyIp: null,
    error: null,
    testTimeMs: 0,
    testing: false,
  });

  // Manual test browser state (currently disabled in UI)
  const [testBrowserId, setTestBrowserId] = useState<string | null>(null);
  const [_testBrowserLoading, setTestBrowserLoading] = useState(false);

  // Save button feedback state
  const [saveStatus, setSaveStatus] = useState<'idle' | 'saving' | 'saved'>('idle');

  // Sync local state when parent config changes (e.g., after loading from backend)
  useEffect(() => {
    setLocalConfig(config);
    setKeywordsText(config.keywords.join(', '));
    // Also sync Google account if loaded from backend
    if (config.accounts?.[0]) {
      setGoogleAccount({
        email: config.accounts[0].email || '',
        password: config.accounts[0].token || '',
        enabled: !!config.accounts[0].email,
      });
    }
  }, [config]);

  // Check if proxy is verified on mount
  useEffect(() => {
    api.isProxyVerified().then(verified => {
      if (verified) {
        setProxyStatus(prev => ({ ...prev, tested: true, working: true }));
      }
    }).catch(() => {});
  }, []);

  // Track previous credentials to detect real changes
  const [prevCreds, setPrevCreds] = useState({
    customer: config.proxyCustomer,
    password: config.proxyPassword,
    country: config.proxyCountry,
  });

  // Also sync prevCreds when config loads from backend
  useEffect(() => {
    setPrevCreds({
      customer: config.proxyCustomer,
      password: config.proxyPassword,
      country: config.proxyCountry,
    });
  }, [config.proxyCustomer, config.proxyPassword, config.proxyCountry]);

  // Only reset proxy status when credentials actually change from user input
  useEffect(() => {
    const credsChanged =
      localConfig.proxyCustomer !== prevCreds.customer ||
      localConfig.proxyPassword !== prevCreds.password ||
      localConfig.proxyCountry !== prevCreds.country;

    if (credsChanged && proxyStatus.tested) {
      setProxyStatus({
        tested: false,
        working: false,
        originalIp: null,
        proxyIp: null,
        error: null,
        testTimeMs: 0,
        testing: false,
      });
      setPrevCreds({
        customer: localConfig.proxyCustomer,
        password: localConfig.proxyPassword,
        country: localConfig.proxyCountry,
      });
    }
  }, [localConfig.proxyCustomer, localConfig.proxyPassword, localConfig.proxyCountry]);

  const handleChange = (field: keyof AppConfig, value: unknown) => {
    setLocalConfig(prev => ({ ...prev, [field]: value }));
  };

  const handleScheduleChange = (field: keyof ScheduleConfig, value: unknown) => {
    setLocalConfig(prev => ({
      ...prev,
      schedule: { ...prev.schedule, [field]: value }
    }));
  };

  const handleDayToggle = (dayIndex: number) => {
    const days = [...localConfig.schedule.days];
    const idx = days.indexOf(dayIndex);
    if (idx >= 0) {
      days.splice(idx, 1);
    } else {
      days.push(dayIndex);
      days.sort();
    }
    handleScheduleChange('days', days);
  };

  const handleTestProxy = async () => {
    if (!localConfig.proxyCustomer || !localConfig.proxyPassword) {
      setProxyStatus({
        tested: true,
        working: false,
        originalIp: null,
        proxyIp: null,
        error: 'Please enter proxy credentials first',
        testTimeMs: 0,
        testing: false,
      });
      return;
    }

    setProxyStatus(prev => ({ ...prev, testing: true, error: null }));

    // First save the config so backend has the credentials
    const keywords = keywordsText
      .split(',')
      .map(k => k.trim())
      .filter(k => k.length > 0);

    try {
      await api.configure({ ...localConfig, keywords });
    } catch (e) {
      setProxyStatus({
        tested: true,
        working: false,
        originalIp: null,
        proxyIp: null,
        error: `Failed to save config: ${e}`,
        testTimeMs: 0,
        testing: false,
      });
      return;
    }

    // Now test the proxy
    try {
      const result = await api.testProxy();
      setProxyStatus({
        tested: true,
        working: result.working,
        originalIp: result.originalIp,
        proxyIp: result.proxyIp,
        error: result.error,
        testTimeMs: result.testTimeMs,
        testing: false,
      });
    } catch (e) {
      setProxyStatus({
        tested: true,
        working: false,
        originalIp: null,
        proxyIp: null,
        error: `Test failed: ${e}`,
        testTimeMs: 0,
        testing: false,
      });
    }
  };

  // Open a manual test browser with proxy (currently disabled in UI)
  const _handleOpenTestBrowser = async () => {
    if (testBrowserId) {
      // Close existing test browser first
      await handleCloseTestBrowser();
    }

    setTestBrowserLoading(true);
    try {
      // Save config first to ensure proxy is configured
      const keywords = keywordsText.split(',').map(k => k.trim()).filter(k => k.length > 0);
      await api.configure({ ...localConfig, keywords });

      const sessionId = await api.openTestBrowser();
      setTestBrowserId(sessionId);
    } catch (e) {
      alert(`Failed to open test browser: ${e}`);
    } finally {
      setTestBrowserLoading(false);
    }
  };

  // Close the manual test browser
  const handleCloseTestBrowser = async () => {
    if (testBrowserId) {
      try {
        await api.closeTestBrowser(testBrowserId);
      } catch (e) {
        console.error('Failed to close test browser:', e);
      }
      setTestBrowserId(null);
    }
  };

  const handleSave = async () => {
    setSaveStatus('saving');

    const keywords = keywordsText
      .split(',')
      .map(k => k.trim())
      .filter(k => k.length > 0);

    // Build accounts array from Google account state
    const accounts: SavedAccount[] = googleAccount.enabled && googleAccount.email
      ? [{
          name: 'Google Account',
          email: googleAccount.email,
          // Note: password is handled separately for security
          token: googleAccount.password, // Temporarily store password in token field for backend
        }]
      : [];

    // Save config with accounts
    try {
      await onSave({ ...localConfig, keywords, accounts });
      setSaveStatus('saved');

      // Reset to idle after 2 seconds
      setTimeout(() => setSaveStatus('idle'), 2000);
    } catch (e) {
      setSaveStatus('idle');
      console.error('Save failed:', e);
    }

    // Auto-test proxy if credentials are provided and not yet tested
    if (localConfig.proxyCustomer && localConfig.proxyPassword && !proxyStatus.tested) {
      // Small delay to let config save first
      setTimeout(() => handleTestProxy(), 500);
    }
  };

  return (
    <div className="config-panel">
      <h2>Configuration</h2>

      <fieldset disabled={disabled}>
        <legend>Proxy Settings (Oxylabs)</legend>
        <div className="form-group">
          <label>Customer ID</label>
          <input
            type="text"
            value={localConfig.proxyCustomer}
            onChange={e => handleChange('proxyCustomer', e.target.value)}
            placeholder="customer-xxxxxx"
          />
        </div>
        <div className="form-group">
          <label>Password</label>
          <input
            type="password"
            value={localConfig.proxyPassword}
            onChange={e => handleChange('proxyPassword', e.target.value)}
            placeholder="Oxylabs password"
          />
        </div>
        <div className="form-group">
          <label>Country</label>
          <select
            value={localConfig.proxyCountry}
            onChange={e => handleChange('proxyCountry', e.target.value)}
          >
            <option value="sa">Saudi Arabia (SA)</option>
            <option value="ae">UAE (AE)</option>
            <option value="us">United States (US)</option>
            <option value="gb">United Kingdom (GB)</option>
          </select>
        </div>
        <div className="form-group">
          <label>Session Time (minutes): {localConfig.proxySesstime || 10}</label>
          <input
            type="range"
            min="1"
            max="30"
            value={localConfig.proxySesstime || 10}
            onChange={e => handleChange('proxySesstime', parseInt(e.target.value))}
          />
          <small className="form-hint">How long to keep the same IP address before rotation</small>
        </div>

        {/* Proxy Test Button and Status */}
        <div className="proxy-test-section">
          <button
            type="button"
            className={`test-proxy-btn ${proxyStatus.testing ? 'testing' : ''}`}
            onClick={handleTestProxy}
            disabled={proxyStatus.testing || !localConfig.proxyCustomer || !localConfig.proxyPassword}
          >
            {proxyStatus.testing ? 'Testing...' : 'Test Proxy Connection'}
          </button>

          {proxyStatus.tested && (
            <div className={`proxy-test-result ${proxyStatus.working ? 'success' : 'error'}`}>
              {proxyStatus.working ? (
                <>
                  <div className="result-header">Proxy Working</div>
                  <div className="result-details">
                    <span>Your IP: {proxyStatus.originalIp || 'Unknown'}</span>
                    <span>Proxy IP: {proxyStatus.proxyIp || 'Unknown'}</span>
                    <span>Time: {proxyStatus.testTimeMs}ms</span>
                  </div>
                </>
              ) : (
                <>
                  <div className="result-header">Proxy Failed</div>
                  <div className="result-error">{proxyStatus.error}</div>
                  {proxyStatus.originalIp && (
                    <div className="result-details">
                      <span>Your IP: {proxyStatus.originalIp}</span>
                    </div>
                  )}
                </>
              )}
            </div>
          )}

          {!proxyStatus.tested && localConfig.proxyCustomer && localConfig.proxyPassword && (
            <div className="proxy-hint">
              Click "Test Proxy Connection" to verify your credentials
            </div>
          )}

          {/* Manual Test Browser - HIDDEN for now (will enable later) */}
          {/*
          <div className="test-browser-section" style={{ marginTop: '15px', paddingTop: '15px', borderTop: '1px solid #333' }}>
            <div style={{ marginBottom: '8px', fontSize: '0.9em', color: '#888' }}>
              Manual Test: Open a browser with proxy to test manually
            </div>
            {!testBrowserId ? (
              <button
                type="button"
                className="test-proxy-btn"
                onClick={handleOpenTestBrowser}
                disabled={testBrowserLoading || !localConfig.proxyCustomer || !localConfig.proxyPassword}
                style={{ backgroundColor: '#2a6a2a' }}
              >
                {testBrowserLoading ? 'Opening...' : 'Open Test Browser'}
              </button>
            ) : (
              <div style={{ display: 'flex', alignItems: 'center', gap: '10px' }}>
                <span style={{ color: '#4a4', fontSize: '0.9em' }}>Test browser open (ID: {testBrowserId.slice(0, 8)}...)</span>
                <button
                  type="button"
                  className="test-proxy-btn"
                  onClick={handleCloseTestBrowser}
                  style={{ backgroundColor: '#6a2a2a', padding: '5px 15px' }}
                >
                  Close
                </button>
              </div>
            )}
          </div>
          */}
        </div>
      </fieldset>

      <fieldset disabled={disabled}>
        <legend>Google Account (Optional)</legend>
        <div className="form-group">
          <label>
            <input
              type="checkbox"
              checked={googleAccount.enabled}
              onChange={e => setGoogleAccount(prev => ({ ...prev, enabled: e.target.checked }))}
            />
            Login to Google before searching
          </label>
          <small className="form-hint">Logged-in users see more personalized ads from the same region</small>
        </div>
        {googleAccount.enabled && (
          <>
            <div className="form-group">
              <label>Google Email</label>
              <input
                type="email"
                value={googleAccount.email}
                onChange={e => setGoogleAccount(prev => ({ ...prev, email: e.target.value }))}
                placeholder="yourname@gmail.com"
              />
            </div>
            <div className="form-group">
              <label>Google Password</label>
              <input
                type="password"
                value={googleAccount.password}
                onChange={e => setGoogleAccount(prev => ({ ...prev, password: e.target.value }))}
                placeholder="Enter password"
              />
              <small className="form-hint">Password is used only for login and not stored</small>
            </div>
            <div className="google-account-note">
              <strong>Note:</strong> Use a Google account that matches the proxy country (Saudi Arabia).
              Accounts with 2-factor authentication may require manual verification.
            </div>
          </>
        )}
      </fieldset>

      <fieldset disabled={disabled}>
        <legend>Session Settings</legend>
        <div className="form-group">
          <label>Concurrent Sessions: {localConfig.concurrentSessions}</label>
          <input
            type="range"
            min="1"
            max="20"
            value={localConfig.concurrentSessions}
            onChange={e => handleChange('concurrentSessions', parseInt(e.target.value))}
          />
        </div>
        <div className="form-group">
          <label>
            <input
              type="checkbox"
              checked={localConfig.headless}
              onChange={e => handleChange('headless', e.target.checked)}
            />
            Headless Mode (invisible browsers)
          </label>
        </div>
        <div className="form-group">
          <label>
            <input
              type="checkbox"
              checked={localConfig.autoRotateIp || false}
              onChange={e => handleChange('autoRotateIp', e.target.checked)}
            />
            Auto-Rotate IP After All Keywords
          </label>
          <small className="form-hint">Restart session with new IP after completing all keyword searches</small>
        </div>
        <div className="form-group">
          <label>Max Clicks per Session: {localConfig.maxClicksPerSession || 0} {localConfig.maxClicksPerSession === 0 || !localConfig.maxClicksPerSession ? '(unlimited)' : ''}</label>
          <input
            type="number"
            min="0"
            max="1000"
            value={localConfig.maxClicksPerSession || 0}
            onChange={e => handleChange('maxClicksPerSession', parseInt(e.target.value) || 0)}
            placeholder="0 = unlimited"
          />
          <small className="form-hint">Session auto-closes after this many clicks (0 = run forever)</small>
        </div>
      </fieldset>

      <fieldset disabled={disabled}>
        <legend>Rate Control</legend>
        <div className="form-group">
          <label>Clicks per Hour (per session): {localConfig.clicksPerHour}</label>
          <input
            type="range"
            min="10"
            max="300"
            value={localConfig.clicksPerHour}
            onChange={e => handleChange('clicksPerHour', parseInt(e.target.value))}
          />
        </div>
        <div className="form-row">
          <div className="form-group">
            <label>Min Delay (sec)</label>
            <input
              type="number"
              min="1"
              max="120"
              value={localConfig.minDelayMs / 1000}
              onChange={e => handleChange('minDelayMs', parseInt(e.target.value) * 1000)}
            />
          </div>
          <div className="form-group">
            <label>Max Delay (sec)</label>
            <input
              type="number"
              min="5"
              max="300"
              value={localConfig.maxDelayMs / 1000}
              onChange={e => handleChange('maxDelayMs', parseInt(e.target.value) * 1000)}
            />
          </div>
        </div>
      </fieldset>

      <fieldset disabled={disabled}>
        <legend>Keywords</legend>
        <div className="form-group">
          <label>Search Keywords (comma-separated)</label>
          <textarea
            value={keywordsText}
            onChange={e => setKeywordsText(e.target.value)}
            placeholder="keyword1, keyword2, keyword3"
            rows={3}
          />
        </div>
      </fieldset>

      <fieldset disabled={disabled}>
        <legend>Schedule</legend>
        <div className="form-group">
          <label>
            <input
              type="checkbox"
              checked={localConfig.schedule.enabled}
              onChange={e => handleScheduleChange('enabled', e.target.checked)}
            />
            Enable Scheduling
          </label>
        </div>
        {localConfig.schedule.enabled && (
          <>
            <div className="form-row">
              <div className="form-group">
                <label>Start Time</label>
                <input
                  type="time"
                  value={localConfig.schedule.startTime}
                  onChange={e => handleScheduleChange('startTime', e.target.value)}
                />
              </div>
              <div className="form-group">
                <label>End Time</label>
                <input
                  type="time"
                  value={localConfig.schedule.endTime}
                  onChange={e => handleScheduleChange('endTime', e.target.value)}
                />
              </div>
            </div>
            <div className="form-group">
              <label>Days</label>
              <div className="days-selector">
                {DAYS.map((day, idx) => (
                  <button
                    key={day}
                    type="button"
                    className={localConfig.schedule.days.includes(idx) ? 'active' : ''}
                    onClick={() => handleDayToggle(idx)}
                  >
                    {day}
                  </button>
                ))}
              </div>
            </div>
          </>
        )}
      </fieldset>

      <button
        className={`save-btn ${saveStatus === 'saved' ? 'saved' : ''}`}
        onClick={handleSave}
        disabled={disabled || saveStatus === 'saving'}
      >
        {saveStatus === 'saving' ? 'Saving...' : saveStatus === 'saved' ? 'Saved!' : 'Save Configuration'}
      </button>
    </div>
  );
}
