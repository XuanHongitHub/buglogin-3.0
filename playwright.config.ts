const config = {
  testDir: './tests/e2e',
  timeout: 30_000,
  fullyParallel: true,
  forbidOnly: Boolean(process.env.CI),
  retries: process.env.CI ? 2 : 0,
  reporter: process.env.CI ? [['line'], ['html', { open: 'never' }]] : [['list']],
  use: { baseURL: process.env.BUGLOGIN_E2E_BASE_URL ?? 'http://127.0.0.1:4310', trace: 'retain-on-failure', video: 'retain-on-failure' },
  webServer: process.env.BUGLOGIN_E2E_SKIP_SERVER === '1' ? undefined : { command: 'cargo run -p buglogin-control-app', url: process.env.BUGLOGIN_E2E_CONTROL_URL ?? 'http://127.0.0.1:4310/v1/health', reuseExistingServer: !process.env.CI, timeout: 30_000 }
};
export default config;
