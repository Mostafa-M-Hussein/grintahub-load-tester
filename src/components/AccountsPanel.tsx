import { useState, useEffect } from 'react';
import type { AppConfig } from '../types';
import * as api from '../services/api';

interface AccountsPanelProps {
  config: AppConfig;
  onConfigChange: (config: AppConfig) => void;
  disabled?: boolean;
}

interface CaptchaStatus {
  configured: boolean;
  balance: number;
  testing: boolean;
  testResult: api.CaptchaTestResult | null;
}

interface Account {
  email: string;
  name: string;
  phone: string | null;
}

export function AccountsPanel({ config, onConfigChange, disabled }: AccountsPanelProps) {
  const [captchaApiKey, setCaptchaApiKey] = useState(config.captchaApiKey || '');
  const [captchaStatus, setCaptchaStatus] = useState<CaptchaStatus>({
    configured: false,
    balance: 0,
    testing: false,
    testResult: null,
  });

  const [accounts, setAccounts] = useState<Account[]>([]);
  const [loadingAccounts, setLoadingAccounts] = useState(false);

  // Registration form
  const [regPassword, setRegPassword] = useState('TestPass123!');
  const [regCount, setRegCount] = useState(1);
  const [registering, setRegistering] = useState(false);
  const [regResult, setRegResult] = useState<{ success: number; failed: number } | null>(null);

  // Load accounts and check captcha on mount
  useEffect(() => {
    loadAccounts();
    if (config.captchaApiKey) {
      checkCaptchaBalance();
    }
  }, []);

  const loadAccounts = async () => {
    setLoadingAccounts(true);
    try {
      const savedAccounts = await api.getSavedAccounts();
      setAccounts(savedAccounts);
    } catch (e) {
      console.error('Failed to load accounts:', e);
    }
    setLoadingAccounts(false);
  };

  const checkCaptchaBalance = async () => {
    try {
      const result = await api.getCaptchaBalance();
      setCaptchaStatus(prev => ({
        ...prev,
        configured: result.configured,
        balance: result.balance,
      }));
    } catch (e) {
      console.error('Failed to check captcha balance:', e);
    }
  };

  const handleSaveApiKey = async () => {
    const newConfig = { ...config, captchaApiKey };
    onConfigChange(newConfig);

    // Check balance after saving
    setTimeout(() => checkCaptchaBalance(), 500);
  };

  const handleTestCaptcha = async () => {
    setCaptchaStatus(prev => ({ ...prev, testing: true, testResult: null }));

    try {
      // Save API key first
      await api.configure({ ...config, captchaApiKey });

      const result = await api.testCaptcha();
      setCaptchaStatus(prev => ({
        ...prev,
        testing: false,
        testResult: result,
      }));

      // Refresh balance after test
      checkCaptchaBalance();
    } catch (e) {
      setCaptchaStatus(prev => ({
        ...prev,
        testing: false,
        testResult: {
          success: false,
          solveTimeMs: 0,
          totalTimeMs: 0,
          tokenPreview: '',
          error: String(e),
        },
      }));
    }
  };

  const handleRegister = async () => {
    setRegistering(true);
    setRegResult(null);

    try {
      // Save config first
      await api.configure({ ...config, captchaApiKey });

      if (regCount === 1) {
        // Single registration
        await api.registerAccount(regPassword);
        setRegResult({ success: 1, failed: 0 });
      } else {
        // Batch registration
        const results = await api.batchRegisterAccounts(regCount, regPassword);
        setRegResult({ success: results.length, failed: regCount - results.length });
      }

      // Reload accounts and balance
      loadAccounts();
      checkCaptchaBalance();
    } catch (e) {
      setRegResult({ success: 0, failed: regCount });
      console.error('Registration failed:', e);
    }

    setRegistering(false);
  };

  const handleDeleteAccount = async (email: string) => {
    try {
      await api.deleteAccount(email);
      setAccounts(prev => prev.filter(a => a.email !== email));
    } catch (e) {
      console.error('Failed to delete account:', e);
    }
  };

  return (
    <div className="accounts-panel">
      <h2>Accounts & CAPTCHA</h2>

      {/* 2Captcha Configuration */}
      <fieldset disabled={disabled}>
        <legend>2Captcha API</legend>
        <div className="form-group">
          <label>API Key</label>
          <div className="input-with-button">
            <input
              type="password"
              value={captchaApiKey}
              onChange={e => setCaptchaApiKey(e.target.value)}
              placeholder="Your 2Captcha API key"
            />
            <button type="button" onClick={handleSaveApiKey}>
              Save
            </button>
          </div>
        </div>

        {captchaStatus.configured && (
          <div className="captcha-info">
            <span className="balance">Balance: ${captchaStatus.balance.toFixed(2)}</span>
          </div>
        )}

        <div className="captcha-test-section">
          <button
            type="button"
            className={`test-btn ${captchaStatus.testing ? 'testing' : ''}`}
            onClick={handleTestCaptcha}
            disabled={captchaStatus.testing || !captchaApiKey}
          >
            {captchaStatus.testing ? 'Solving CAPTCHA...' : 'Test CAPTCHA Solving'}
          </button>

          {captchaStatus.testResult && (
            <div className={`test-result ${captchaStatus.testResult.success ? 'success' : 'error'}`}>
              {captchaStatus.testResult.success ? (
                <>
                  <div className="result-header">CAPTCHA Solved!</div>
                  <div className="result-details">
                    <span>Solve time: {(captchaStatus.testResult.solveTimeMs / 1000).toFixed(1)}s</span>
                    <span>Token: {captchaStatus.testResult.tokenPreview}...</span>
                  </div>
                </>
              ) : (
                <>
                  <div className="result-header">CAPTCHA Failed</div>
                  <div className="result-error">{captchaStatus.testResult.error}</div>
                </>
              )}
            </div>
          )}
        </div>
      </fieldset>

      {/* Account Registration */}
      <fieldset disabled={disabled || !captchaApiKey}>
        <legend>Register Accounts</legend>
        <div className="form-row">
          <div className="form-group">
            <label>Password</label>
            <input
              type="text"
              value={regPassword}
              onChange={e => setRegPassword(e.target.value)}
              placeholder="Password for new accounts"
            />
          </div>
          <div className="form-group">
            <label>Count</label>
            <input
              type="number"
              min="1"
              max="10"
              value={regCount}
              onChange={e => setRegCount(parseInt(e.target.value) || 1)}
            />
          </div>
        </div>

        <button
          type="button"
          className={`register-btn ${registering ? 'registering' : ''}`}
          onClick={handleRegister}
          disabled={registering || !captchaApiKey || !regPassword}
        >
          {registering ? `Registering... (${regCount} accounts)` : `Register ${regCount} Account${regCount > 1 ? 's' : ''}`}
        </button>

        {regResult && (
          <div className={`reg-result ${regResult.failed === 0 ? 'success' : regResult.success > 0 ? 'partial' : 'error'}`}>
            {regResult.success > 0 && <span className="success-count">{regResult.success} created</span>}
            {regResult.failed > 0 && <span className="failed-count">{regResult.failed} failed</span>}
          </div>
        )}

        <p className="hint">
          Each registration takes ~15-20s (CAPTCHA solving time). Accounts are auto-saved.
        </p>
      </fieldset>

      {/* Saved Accounts List */}
      <fieldset>
        <legend>Saved Accounts ({accounts.length})</legend>
        {loadingAccounts ? (
          <div className="loading">Loading accounts...</div>
        ) : accounts.length === 0 ? (
          <div className="no-accounts">No accounts yet. Register some above.</div>
        ) : (
          <div className="accounts-list">
            {accounts.map(account => (
              <div key={account.email} className="account-item">
                <div className="account-info">
                  <span className="email">{account.email}</span>
                  <span className="name">{account.name}</span>
                  {account.phone && <span className="phone">{account.phone}</span>}
                </div>
                <button
                  type="button"
                  className="delete-btn"
                  onClick={() => handleDeleteAccount(account.email)}
                  title="Delete account"
                >
                  X
                </button>
              </div>
            ))}
          </div>
        )}
      </fieldset>
    </div>
  );
}
