import assert from 'node:assert/strict'
import { execFileSync } from 'node:child_process'
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import path from 'node:path'
import test from 'node:test'

const tool = new URL('./macos-source-receipt.mjs', import.meta.url)
const installer = readFileSync(new URL('./install-macos-dev-app.sh', import.meta.url), 'utf8')
const delivery = readFileSync(new URL('./prepare-macos-delivery.sh', import.meta.url), 'utf8')

function runTool(repoRoot, appPath, args = [], env = {}) {
  return JSON.parse(execFileSync(process.execPath, [tool.pathname, '--repo-root', repoRoot, '--app', appPath, ...args], {
    encoding: 'utf8',
    env: { ...process.env, ...env },
    stdio: ['ignore', 'pipe', 'pipe'],
  }))
}

function makeFixture() {
  const root = mkdtempSync(path.join(tmpdir(), 'cunzhi-source-receipt-'))
  const appPath = path.join(root, 'target/release/bundle/macos/iterate.app')
  mkdirSync(path.join(appPath, 'Contents/Resources'), { recursive: true })
  writeFileSync(path.join(root, '.gitignore'), 'target/\n')
  writeFileSync(path.join(root, 'tracked.txt'), 'one\n')
  execFileSync('git', ['init', '-q'], { cwd: root })
  execFileSync('git', ['add', '.gitignore', 'tracked.txt'], { cwd: root })
  execFileSync('git', ['-c', 'user.name=Test', '-c', 'user.email=test@example.com', 'commit', '-qm', 'fixture'], { cwd: root })
  return { root, appPath }
}

test('writes and verifies a clean receipt tied to the current commit and tree', () => {
  const fixture = makeFixture()
  try {
    const written = runTool(fixture.root, fixture.appPath)
    assert.equal(written.status, 'written')
    assert.equal(written.receipt.dirty, false)
    assert.equal(written.receipt.dirty_snapshot_sha256, null)
    assert.match(written.receipt.source_commit, /^[a-f0-9]{40}$/)
    assert.match(written.receipt.source_tree, /^[a-f0-9]{40}$/)
    assert.match(written.receipt.worktree_snapshot_sha256, /^[a-f0-9]{64}$/)

    const verified = runTool(fixture.root, fixture.appPath, ['--verify'])
    assert.equal(verified.status, 'verified')
    assert.equal(verified.receipt.source_commit, written.receipt.source_commit)
  }
  finally {
    rmSync(fixture.root, { recursive: true, force: true })
  }
})

test('refuses dirty relabeling unless explicitly allowed and fingerprints the dirty snapshot', () => {
  const fixture = makeFixture()
  try {
    writeFileSync(path.join(fixture.root, 'tracked.txt'), 'two\n')
    assert.throws(
      () => runTool(fixture.root, fixture.appPath),
      /refusing to create a source receipt from a dirty worktree/,
    )

    const dirty = runTool(fixture.root, fixture.appPath, [], { CUNZHI_MACOS_ALLOW_DIRTY_SOURCE: '1' })
    assert.equal(dirty.receipt.dirty, true)
    assert.match(dirty.receipt.dirty_snapshot_sha256, /^[a-f0-9]{64}$/)
  }
  finally {
    rmSync(fixture.root, { recursive: true, force: true })
  }
})

test('installer writes only after a fresh build and verifies receipts for skip-build installs', () => {
  assert.match(installer, /write_or_verify_source_receipt/)
  assert.match(installer, /if \[\[ "\$\{DO_BUILD\}" -eq 1 \]\]; then[\s\S]*?node "\$\{SOURCE_RECEIPT_TOOL\}" --repo-root "\$\{REPO_ROOT\}" --app "\$\{SOURCE_APP\}"/)
  assert.match(installer, /else[\s\S]*?node "\$\{SOURCE_RECEIPT_TOOL\}" --verify --repo-root "\$\{REPO_ROOT\}" --app "\$\{SOURCE_APP\}"/)
  assert.match(installer, /node "\$\{SOURCE_RECEIPT_TOOL\}" --verify --repo-root "\$\{REPO_ROOT\}" --app "\$\{DEST_APP\}"/)
})

test('delivery writes a clean receipt before final signing and verifies it before packaging', () => {
  assert.match(delivery, /SOURCE_RECEIPT_TOOL="\$\{REPO_ROOT\}\/scripts\/macos-source-receipt\.mjs"/)

  const pruneIndex = delivery.indexOf('\nprune_release_bundle\n')
  const writeIndex = delivery.indexOf('node "${SOURCE_RECEIPT_TOOL}" --repo-root "${REPO_ROOT}" --app "${APP_PATH}"')
  const signIndex = delivery.indexOf('sign_app_bundle "${SIGN_IDENTITY}"')
  const verifyIndex = delivery.indexOf('node "${SOURCE_RECEIPT_TOOL}" --verify --repo-root "${REPO_ROOT}" --app "${APP_PATH}"')
  const packageIndex = delivery.indexOf('ditto -c -k --sequesterRsrc --keepParent "${APP_PATH}"')

  assert.ok(pruneIndex >= 0 && pruneIndex < writeIndex, 'receipt must be written after bundle cleanup')
  assert.ok(writeIndex < signIndex, 'receipt must be written before final signing')
  assert.ok(signIndex < verifyIndex, 'signed bundle must retain the same receipt')
  assert.ok(verifyIndex < packageIndex, 'receipt must be verified before packaging')
})

test('installer creates and verifies a canonical GUI Fn owner after replacement', () => {
  assert.match(installer, /open -n "\$\{DEST_APP\}"/)
  assert.match(installer, /wait_for_canonical_gui_owner/)
  assert.match(installer, /canonical-gui/)
  assert.match(installer, /fn-owner\.lock/)
  assert.doesNotMatch(installer, /info "Opening installed app"\s+open "\$\{DEST_APP\}"/)
})

test('installer preserves bridge and relay by default and restarts only explicitly requested services', () => {
  assert.match(installer, /RESTART_BRIDGE=0/)
  assert.match(installer, /RESTART_RELAY=0/)
  assert.doesNotMatch(installer, /RESTART_PRESERVED_BACKGROUND=1/)
  assert.match(installer, /--restart-bridge\)/)
  assert.match(installer, /--restart-relay\)/)
  assert.match(installer, /restart_requested_background_processes/)
  assert.match(installer, /if \[\[ "\$\{RESTART_BRIDGE\}" -eq 1 \]\]/)
  assert.match(installer, /if \[\[ "\$\{RESTART_RELAY\}" -eq 1 \]\]/)
  assert.match(installer, /launchctl kickstart -k "\$\{service\}"/)
})

test('installer stops only the canonical Fn owner unless background shutdown is explicit', () => {
  assert.match(installer, /should_stop_app_process/)
  assert.doesNotMatch(installer, /is_background_app_process/)

  const stopCallSites = installer.match(/if should_stop_app_process "\$\{pid\}"; then/g) ?? []
  assert.equal(stopCallSites.length, 1, 'only the installed (dest) bundle scan may stop processes')
  assert.match(installer, /pid_in_dest_bundle "\$\{pid\}"/)
  const foreignSource = installer.match(/warn_about_foreign_app_bundles\(\) \{[\s\S]*?\n\}/)?.[0]
  assert.ok(foreignSource, 'expected a warn-only scan of other app bundles')
  assert.doesNotMatch(foreignSource, /\bkill\b/)
  assert.doesNotMatch(installer, /stop_conflicting_foreground_app_bundles/)

  const canonicalSource = installer.match(/is_canonical_gui_process\(\) \{[\s\S]*?\n\}/)?.[0]
  assert.ok(canonicalSource, 'expected an owner-backed canonical GUI classifier')
  assert.match(canonicalSource, /fn_owner_lock_for_pid/)
  assert.match(canonicalSource, /fn_owner_metadata_matches/)

  const functionSource = installer.match(/should_stop_app_process\(\) \{[\s\S]*?\n\}/)?.[0]
  assert.ok(functionSource, 'expected a testable should_stop_app_process shell function')

  const output = execFileSync('bash', ['-c', `
${functionSource}
is_canonical_gui_process() {
  [[ "$1" == "101" ]]
}
probe() {
  if should_stop_app_process "$1"; then
    printf '%s=stop\\n' "$2"
  else
    printf '%s=preserve\\n' "$2"
  fi
}
STOP_BACKGROUND=0
probe 101 canonical
probe 202 serve
probe 303 standalone_popup
STOP_BACKGROUND=1
probe 202 explicit_background_shutdown
`], { encoding: 'utf8' })

  assert.equal(output, [
    'canonical=stop',
    'serve=preserve',
    'standalone_popup=preserve',
    'explicit_background_shutdown=stop',
    '',
  ].join('\n'))
})

test('installer keeps the previous signed bundle while preserved processes still use it', () => {
  assert.match(installer, /bundle_has_running_code\(\)/)
  assert.match(installer, /codesign -h "\$\{pid\}"/)
  assert.match(installer, /cleanup_retired_apps "\$\{retired_root\}"/)
  assert.match(installer, /mv "\$\{DEST_APP\}" "\$\{retired_app\}"/)
  assert.match(installer, /mv "\$\{staging_app\}" "\$\{DEST_APP\}"/)
  assert.match(installer, /Preserving retired bundle for running signed processes/)
  assert.doesNotMatch(installer, /rm -rf "\$\{DEST_APP\}"/)

  const retireIndex = installer.indexOf('mv "${DEST_APP}" "${retired_app}"')
  const installIndex = installer.indexOf('mv "${staging_app}" "${DEST_APP}"')
  assert.ok(retireIndex >= 0 && retireIndex < installIndex, 'the old bundle must move away before the staged bundle takes its canonical path')
})
