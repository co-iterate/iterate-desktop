import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { existsSync, mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import test from 'node:test'

// These exercise Bash control flow with fake OS commands, not macOS/TCC behavior.
const bash = process.env.BASH_TEST_EXECUTABLE || 'bash'
const available = spawnSync(bash, ['--version']).status === 0
const script = readFileSync(new URL('./install-macos-dev-app.sh', import.meta.url), 'utf8')
const definitions = script.slice(script.indexOf('APP_NAME='), script.indexOf('while [[ "$#" -gt 0 ]]'))
const preserveProfilePython = script.match(/<<'PY'\r?\n([\s\S]*?)\r?\nPY/)[1]
const run = (body, options = {}) => spawnSync(bash, ['-s'], {
  input: `set -euo pipefail\nREPO_ROOT="$PWD"\n${definitions}\n${body}`,
  encoding: 'utf8', ...options,
  env: { ...process.env, CUNZHI_MACOS_DEV_SIGN_IDENTITY: '', CUNZHI_MACOS_ALLOW_ADHOC_SIGN: '0' },
})
const setup = `
DEST_APP=.
ENTITLEMENTS_PATH="$PWD/Entitlements.plist"
security() { printf '%s\\n' '  1) ABC "Developer ID Application: Example (TEAM)"' '  2) DEF "Apple Development: Example (TEAM)"'; }
codesign() {
  if [[ "$*" == *-r-* ]]; then echo 'designated => identifier "example"';
  else echo 'Authority=Apple Development: Example (TEAM)'; fi
}
`

for (const profileMode of ['present', 'absent', 'invalid']) {
  test(`preserves custom installed identity and canonical launcher with profile ${profileMode}`, () => {
    const root = mkdtempSync(path.join(os.tmpdir(), 'iterate-profile-test-'))
    try {
      const result = spawnSync(process.env.PYTHON_TEST_EXECUTABLE || (process.platform === 'win32' ? 'py' : 'python3'), ['-c', `
import json, pathlib, plistlib, sys
root = pathlib.Path(sys.argv[1])
old, staged = root / 'installed.app', root / 'staged.app'
for app in (old, staged):
    (app / 'Contents/MacOS').mkdir(parents=True)
    (app / 'Contents/Resources').mkdir()
    info = {'CFBundleIdentifier': 'dev.iterate.cross-device' if app == old else 'com.kexin94yyds.iterate', 'CFBundleExecutable': 'iterate'}
    (app / 'Contents/Info.plist').write_bytes(plistlib.dumps(info))
(old / 'Contents/MacOS/launcher').write_text('#!/bin/sh\\nexec "$(dirname "$0")/iterate" "$@"\\n# never copy arbitrary commands\\n')
expected_profile = {'ITERATE_CONFIG_DIR': '/Users/parker/.config/iterate', 'ITERATE_CROSS_DEVICE_DIR': '/Users/parker/.iterate-cross-device'}
mode = sys.argv[2]
if mode != 'absent':
    (old / 'Contents/Resources/iterate-profile.json').write_text(json.dumps(expected_profile if mode == 'present' else []))
sys.argv = ['preserve', str(old), str(staged), 'iterate']
try:
    exec(${JSON.stringify(preserveProfilePython)})
except SystemExit as error:
    assert mode == 'invalid' and 'Invalid installed profile' in str(error), str(error)
else:
    assert mode != 'invalid', 'invalid profile was accepted'
    info = plistlib.loads((staged / 'Contents/Info.plist').read_bytes())
    assert info['CFBundleIdentifier'] == 'dev.iterate.cross-device'
    assert info['CFBundleExecutable'] == 'iterate'
    assert (staged / 'Contents/MacOS/launcher').read_text() == '#!/bin/sh\\nexec "$(dirname "$0")/iterate" "$@"\\n'
    preserved = staged / 'Contents/Resources/iterate-profile.json'
    if mode == 'present':
        assert json.loads(preserved.read_text()) == expected_profile
    else:
        assert not preserved.exists()
`, root, profileMode], { encoding: 'utf8' })
      assert.equal(result.status, 0, result.error?.message || result.stderr || result.stdout)
    }
    finally { rmSync(root, { recursive: true, force: true }) }
  })
}
for (const [name, body, status, pattern] of [
  ['reuses installed development identity ahead of Developer ID', `${setup}\nprepare_sign_identity\necho "selected=$SIGN_IDENTITY requirement=$INSTALLED_REQUIREMENT"`, 0, /selected=DEF requirement=identifier "example"/],
  ['missing installed certificate refuses automatic identity change', `${setup}\nsecurity() { echo '1) ABC "Developer ID Application: Example (TEAM)"'; }\nprepare_sign_identity`, 1, /matching signing certificate unavailable/],
  ['fresh install prefers stable Developer ID', `${setup}\nDEST_APP=/nonexistent-iterate-test.app\nprepare_sign_identity\necho "$SIGN_IDENTITY"`, 0, /Developer ID Application: Example/],
  ['missing all certificates refuses silent ad-hoc fallback', `${setup}\nDEST_APP=/nonexistent-iterate-test.app\nsecurity() { :; }\nprepare_sign_identity`, 1, /no stable signing identity/],
  ['explicit ad-hoc fallback is retained', `${setup}\nsecurity() { :; }\nCUNZHI_MACOS_ALLOW_ADHOC_SIGN=1\nprepare_sign_identity\necho "selected=$SIGN_IDENTITY"`, 0, /selected=-/],
  ['explicit identity bypasses auto-selection', `${setup}\nSIGN_IDENTITY=-\nsecurity() { return 1; }\nprepare_sign_identity\necho "selected=$SIGN_IDENTITY"`, 0, /selected=-/],
  ['no-sign needs no keychain access', `${setup}\nDO_SIGN=0\nsecurity() { return 1; }\nprepare_sign_identity`, 0, /--no-sign may invalidate/],
]) {
  test(name, { skip: !available }, () => {
    const result = run(body)
    assert.equal(result.status, status, result.stderr + result.stdout)
    assert.match(result.stdout + result.stderr, pattern)
  })
}

for (const failure of ['launcher-sign', 'binary-sign', 'bundle-sign', 'verify', 'requirement', 'none']) {
  test(`staging transaction: ${failure}`, { skip: !available }, () => {
    const root = mkdtempSync(path.join(os.tmpdir(), 'iterate-sign-test-'))
    for (const dir of ['source.app/Contents/MacOS', 'installed.app/Contents/MacOS']) mkdirSync(path.join(root, dir), { recursive: true })
    writeFileSync(path.join(root, 'source.app/Contents/MacOS/iterate'), 'new')
    writeFileSync(path.join(root, 'source.app/Contents/MacOS/mcp-server'), 'helper')
    writeFileSync(path.join(root, 'source.app/Contents/MacOS/launcher'), '#!/bin/sh\nexec ./iterate "$@"\n')
    writeFileSync(path.join(root, 'installed.app/Contents/MacOS/iterate'), 'old')
    writeFileSync(path.join(root, 'entitlements.plist'), 'fixture')
    try {
      const result = run(`
SOURCE_APP="$PWD/source.app"
DEST_APP="$PWD/installed.app"
ENTITLEMENTS_PATH="$PWD/entitlements.plist"
SIGN_IDENTITY=ABC
INSTALLED_REQUIREMENT='identifier "example"'
SIGN_TIMESTAMP=none
cleanup_retired_apps() { :; }
preserve_installed_profile() { :; }
bundle_has_running_code() { return 1; }
ditto() { cp -R "$1" "$2"; }
xattr() { :; }
file() { echo Mach-O; }
codesign() {
  echo "$*" >> "$PWD/sign.log"
  case '${failure}: '"$*" in
    launcher-sign:*--force*Contents/MacOS/launcher) return 1 ;;
    binary-sign:*--force*Contents/MacOS/mcp-server) return 1 ;;
    bundle-sign:*--entitlements*) return 1 ;;
    verify:*--deep*) return 1 ;;
    requirement:*-R*) return 1 ;;
  esac
}
copy_app
`, { cwd: root })
      assert.equal(result.status, failure === 'none' ? 0 : 1, result.stderr + result.stdout)
      assert.equal(readFileSync(path.join(root, 'installed.app/Contents/MacOS/iterate'), 'utf8'), failure === 'none' ? 'new' : 'old')
      if (failure !== 'none') assert.equal(existsSync(path.join(root, '.iterate-retired')), false)
      const log = readFileSync(path.join(root, 'sign.log'), 'utf8')
      assert.match(log, /\.iterate-installing-/)
      assert.doesNotMatch(log, /installed\.app/)
      if (failure === 'none') {
        const calls = log.trim().split('\n')
        assert.match(calls[0], /Contents\/MacOS\/launcher$/)
        assert.match(calls[1], /Contents\/MacOS\/mcp-server$/)
        assert.match(calls[2], /--entitlements/)
        assert.doesNotMatch(log, /--force[^\n]*Contents\/MacOS\/iterate$/m)
        assert.match(log, /-R =identifier "example"/)
      }
    }
    finally { rmSync(root, { recursive: true, force: true }) }
  })
}
