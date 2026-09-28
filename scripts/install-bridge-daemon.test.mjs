import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import nodeTest from 'node:test';

const test = process.platform === 'win32' ? nodeTest.skip : nodeTest;

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(__dirname, '..');
const installScript = path.join(repoRoot, 'scripts/install-bridge-daemon.sh');

function makeTempWorkspace() {
  return fs.mkdtempSync(path.join(os.tmpdir(), 'install-bridge-daemon-test-'));
}

function writeExecutable(filePath, content) {
  fs.writeFileSync(filePath, content, { mode: 0o755 });
}

function baseEnvironment(workspace, appBin, overrides = {}) {
  return {
    ...process.env,
    HOME: workspace,
    APP_BIN: appBin,
    WORKSPACE_PATH: workspace,
    LABEL: 'com.cunzhi.iterate.bridge.test',
    PORT: '18080',
    APNS_KEY_ID: '',
    APNS_TEAM_ID: '',
    APNS_AUTH_KEY_PATH: '',
    APNS_TOPIC: '',
    APNS_ENV: '',
    ...overrides,
  };
}

test('render persists APNs environment from apns-env.sh into LaunchAgent plist', () => {
  const workspace = makeTempWorkspace();
  const appBin = path.join(workspace, 'iterate');
  writeExecutable(appBin, '#!/usr/bin/env bash\necho iterate\n');
  const configDir = path.join(workspace, '.config/iterate');
  fs.mkdirSync(configDir, { recursive: true });
  fs.writeFileSync(path.join(configDir, 'apns-env.sh'), [
    'export APNS_KEY_ID="FILE_KEY"',
    'export APNS_TEAM_ID="FILE_TEAM"',
    `export APNS_AUTH_KEY_PATH="${workspace}/AuthKey_FILE.p8"`,
    'export APNS_TOPIC="com.iterate.notify.file"',
    'export APNS_ENV="sandbox"',
    '',
  ].join('\n'));

  const result = spawnSync('bash', [installScript, 'render'], {
    cwd: repoRoot,
    encoding: 'utf8',
    env: baseEnvironment(workspace, appBin),
  });

  assert.equal(result.status, 0, result.stderr || result.stdout);
  assert.match(result.stdout, /<key>APNS_KEY_ID<\/key>\s*<string>FILE_KEY<\/string>/);
  assert.match(result.stdout, /<key>APNS_TEAM_ID<\/key>\s*<string>FILE_TEAM<\/string>/);
  assert.match(result.stdout, /<key>APNS_AUTH_KEY_PATH<\/key>/);
  assert.match(result.stdout, new RegExp(`${workspace}/AuthKey_FILE\\.p8`.replaceAll('/', '\\/')));
  assert.match(result.stdout, /<key>APNS_TOPIC<\/key>\s*<string>com\.iterate\.notify\.file<\/string>/);
  assert.match(result.stdout, /<key>APNS_ENV<\/key>\s*<string>sandbox<\/string>/);
});

test('install refuses an untrusted Bridge identity before replacing the plist', () => {
  const workspace = makeTempWorkspace();
  const appBin = path.join(workspace, 'iterate');
  const fakeBin = path.join(workspace, 'bin');
  const plistPath = path.join(workspace, 'bridge.plist');
  fs.mkdirSync(fakeBin, { recursive: true });
  writeExecutable(appBin, '#!/usr/bin/env bash\necho iterate\n');
  writeExecutable(path.join(fakeBin, 'codesign'), '#!/usr/bin/env bash\nexit 1\n');
  fs.writeFileSync(plistPath, 'existing-plist');

  const result = spawnSync('bash', [installScript, 'install'], {
    cwd: repoRoot,
    encoding: 'utf8',
    env: baseEnvironment(workspace, appBin, {
      PATH: `${fakeBin}:${process.env.PATH}`,
      PLIST_PATH: plistPath,
    }),
  });

  assert.equal(result.status, 1, result.stderr || result.stdout);
  assert.match(result.stderr, /does not satisfy the iterate Developer ID requirement/);
  assert.equal(fs.readFileSync(plistPath, 'utf8'), 'existing-plist');
});

test('install accepts a Bridge identity that satisfies the broker requirement', () => {
  const workspace = makeTempWorkspace();
  const appBin = path.join(workspace, 'iterate');
  const fakeBin = path.join(workspace, 'bin');
  const codesignLog = path.join(workspace, 'codesign.log');
  const plistPath = path.join(workspace, 'bridge.plist');
  fs.mkdirSync(fakeBin, { recursive: true });
  writeExecutable(appBin, '#!/usr/bin/env bash\necho iterate\n');
  writeExecutable(path.join(fakeBin, 'codesign'), `#!/usr/bin/env bash\nprintf '%s\\n' "$@" > "${codesignLog}"\n`);

  const result = spawnSync('bash', [installScript, 'install'], {
    cwd: repoRoot,
    encoding: 'utf8',
    env: baseEnvironment(workspace, appBin, {
      PATH: `${fakeBin}:${process.env.PATH}`,
      PLIST_PATH: plistPath,
    }),
  });

  assert.equal(result.status, 0, result.stderr || result.stdout);
  const installed = fs.readFileSync(plistPath, 'utf8');
  assert.match(installed, new RegExp(appBin.replaceAll('/', '\\/')));
  assert.match(installed, /<string>--bridge-only<\/string>/);
  const codesignArguments = fs.readFileSync(codesignLog, 'utf8');
  assert.match(codesignArguments, /^--verify$/m);
  assert.match(codesignArguments, /^--strict$/m);
  assert.match(codesignArguments, /^-R=identifier "com\.kexin94yyds\.iterate" and anchor apple generic/m);
  assert.match(codesignArguments, /certificate leaf\[subject\.OU\] = "UM3Z9G5DNH"/);
});

test('restart rejects an untrusted candidate before calling launchctl', () => {
  const workspace = makeTempWorkspace();
  const appBin = path.join(workspace, 'iterate');
  const fakeBin = path.join(workspace, 'bin');
  const launchctlLog = path.join(workspace, 'launchctl.log');
  fs.mkdirSync(fakeBin, { recursive: true });
  writeExecutable(appBin, '#!/usr/bin/env bash\necho iterate\n');
  writeExecutable(path.join(fakeBin, 'codesign'), '#!/usr/bin/env bash\nexit 1\n');
  writeExecutable(path.join(fakeBin, 'launchctl'), `#!/usr/bin/env bash\nprintf '%s\\n' "$*" >> "${launchctlLog}"\n`);

  const result = spawnSync('bash', [installScript, 'restart'], {
    cwd: repoRoot,
    encoding: 'utf8',
    env: baseEnvironment(workspace, appBin, {
      PATH: `${fakeBin}:${process.env.PATH}`,
    }),
  });

  assert.equal(result.status, 1, result.stderr || result.stdout);
  assert.match(result.stderr, /does not satisfy the iterate Developer ID requirement/);
  assert.equal(fs.existsSync(launchctlLog), false);
});
