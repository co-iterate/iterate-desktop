import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'
import { mkdtemp, mkdir, readFile, rm, symlink } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import path from 'node:path'
import test from 'node:test'

import {
  auditLicenseDigest,
  auditPackageScriptFiles,
  auditPackageScripts,
  auditSourceAssetPolicy,
  auditWorkflowActionPins,
  scanTextFindings,
  selectIncludedPaths,
  validateExportDestination,
} from './export-desktop-oss.mjs'

const manifest = JSON.parse(await readFile(new URL('../open-source-manifest.json', import.meta.url), 'utf8'))

function windowsDriveUserPath(user) {
  return ['C:', 'Users', user, 'project'].join('\\')
}

function windowsUncUserPath(user) {
  return `\\\\${['fileserver', 'Users', user, 'project'].join('\\')}`
}

function pngDimensions(bytes) {
  assert.equal(bytes.subarray(1, 4).toString('ascii'), 'PNG')
  return {
    width: bytes.readUInt32BE(16),
    height: bytes.readUInt32BE(20),
  }
}

function sha256(bytes) {
  return createHash('sha256').update(bytes).digest('hex')
}

test('public README uses coiterate ownership and current product and community images', async () => {
  const readme = await readFile(new URL('../README.md', import.meta.url), 'utf8')
  const windowsSetup = await readFile(new URL('../WINDOWS_SETUP_FOR_AI.md', import.meta.url), 'utf8')
  const hero = await readFile(new URL('../assets/iterate-desktop-hero-zh.png', import.meta.url))
  const desktop = await readFile(new URL('../assets/iterate-desktop-interceptor-zh.png', import.meta.url))
  const mobile = await readFile(new URL('../assets/iterate-mobile-interceptor-zh.png', import.meta.url))
  const wechat = await readFile(new URL('../assets/community-wechat-group.png', import.meta.url))
  const qq = await readFile(new URL('../assets/community-qq-group.png', import.meta.url))

  assert.match(readme, /github\.com\/co-iterate\/iterate-desktop/)
  assert.match(readme, /由 <a href="https:\/\/github\.com\/co-iterate">coiterate<\/a> 共同维护/)
  assert.match(readme, /assets\/iterate-desktop-hero-zh\.png/)
  assert.match(readme, /assets\/iterate-desktop-interceptor-zh\.png/)
  assert.match(readme, /assets\/iterate-mobile-interceptor-zh\.png/)
  assert.match(readme, /assets\/community-wechat-group\.png/)
  assert.match(readme, /assets\/community-qq-group\.png/)
  assert.match(readme, /有效期至 2026-09-04/)
  assert.match(readme, /群号：186107551/)
  assert.match(readme, /CONTRIBUTING\.md/)
  assert.match(readme, /Android 远程继续[\s\S]*?设计中[\s\S]*?github\.com\/co-iterate\/iterate-desktop\/issues\/22[\s\S]*?尚无可下载 APK/)
  assert.doesNotMatch(readme, /star-history\.com/)
  assert.doesNotMatch(readme, /community-qr-publisher|管理员合并|内部维护流程/)
  assert.doesNotMatch(readme, /github\.com\/kexin94yyds\/iterate-desktop/)
  assert.match(windowsSetup, /github\.com\/co-iterate\/iterate-desktop\.git/)
  assert.doesNotMatch(windowsSetup, /github\.com\/kexin94yyds\/iterate-desktop/)
  assert.deepEqual(pngDimensions(hero), { width: 2326, height: 914 })
  assert.deepEqual(pngDimensions(desktop), { width: 1200, height: 1606 })
  assert.deepEqual(pngDimensions(mobile), { width: 1284, height: 2778 })
  assert.equal(sha256(mobile), '4c93e6eb6e937e7ae5ed01dc6ec57bc835ecb5151d1a6b842443898b8085affb')
  assert.deepEqual(pngDimensions(wechat), { width: 850, height: 850 })
  assert.deepEqual(pngDimensions(qq), { width: 981, height: 981 })
})

test('desktop allowlist includes the buildable core and excludes private clients and artifacts', () => {
  const selected = selectIncludedPaths(manifest, [
    'src/rust/lib.rs',
    'src/frontend/main.ts',
    'src/bin/mcp-server.rs',
    'scripts/release-sign-assets.mjs',
    'scripts/desktop-codex-live-source.test.mjs',
    'Cargo.toml',
    'LICENSE',
    'open-source-manifest.json',
    'ios-app/IterateNotify/ContentView.swift',
    'browser-extension/background.js',
    'vscode-extension/src/extension.ts',
    '.cunzhi-memory/context.md',
    'app-main-BnTSnuSB.js',
    'release-package/windsurf-cunzhi',
    'scripts/xhs_ai_weekly_report.mjs',
  ])

  assert.deepEqual(selected, [
    'Cargo.toml',
    'LICENSE',
    'open-source-manifest.json',
    'scripts/desktop-codex-live-source.test.mjs',
    'scripts/release-sign-assets.mjs',
    'src/bin/mcp-server.rs',
    'src/frontend/main.ts',
    'src/rust/lib.rs',
  ])
})

test('desktop source excludes uncleared audio while preserving the empty resource directory', () => {
  const selected = selectIncludedPaths(manifest, [
    'src/rust/assets/resources/README.md',
    'src/rust/assets/resources/level-up-191997[level-up-191997].mp3',
    'src/rust/assets/resources/mixkit-correct-answer-tone-2870[mixkit-correct-answer-tone-2870].wav',
  ])

  assert.deepEqual(selected, ['src/rust/assets/resources/README.md'])
})

test('desktop source gate rejects future audio even when its bytes look textual', () => {
  assert.deepEqual(
    auditSourceAssetPolicy('src/rust/assets/resources/future-tone.mp3'),
    [{
      code: 'audio-source-file-not-allowed',
      path: 'src/rust/assets/resources/future-tone.mp3',
    }],
  )
  assert.deepEqual(
    auditSourceAssetPolicy('src/rust/assets/resources/README.md'),
    [],
  )
})

test('sensitive content findings never echo matched secret material', () => {
  const privateKey = [
    '-----BEGIN',
    'PRIVATE KEY-----',
    'TEST',
    '-----END PRIVATE KEY-----',
  ].join('\n').replace('BEGIN\nPRIVATE', 'BEGIN PRIVATE')
  const personalPath = ['/Users', 'private-person', 'project'].join('/')
  const windowsPath = windowsDriveUserPath('private-person')
  const uncPath = windowsUncUserPath('private-person')
  const findings = scanTextFindings(
    'src/example.rs',
    [
      `const KEY: &str = ${JSON.stringify(privateKey)};`,
      `const PATH: &str = ${JSON.stringify(personalPath)};`,
      `WINDOWS_PATH=${windowsPath}`,
      `UNC_PATH=${uncPath}`,
    ].join('\n'),
  )

  assert.deepEqual(findings.map(finding => finding.code), [
    'private-key-material',
    'personal-absolute-path',
    'personal-absolute-path',
    'personal-absolute-path',
  ])
  assert.ok(findings.every(finding => !JSON.stringify(finding).includes('TEST')))
  assert.ok(findings.every(finding => !JSON.stringify(finding).includes('private-person')))
})

test('GitHub Actions must use immutable SHAs while local actions remain allowed', () => {
  assert.deepEqual(
    auditWorkflowActionPins('.github/workflows/ci.yml', `
steps:
  - uses: actions/checkout@v4
  - uses: owner/action@0123456789abcdef0123456789abcdef01234567 # v1
  - uses: ./actions/local
`),
    [{ code: 'unpinned-action', path: '.github/workflows/ci.yml', line: 3 }],
  )
})

test('Windows path scanning catches raw and source-escaped user directories', () => {
  const privateDrive = windowsDriveUserPath('private-person')
  const privateUnc = windowsUncUserPath('private-person')
  const privatePaths = [
    privateDrive,
    `const p = ${JSON.stringify(privateDrive)};`,
    privateUnc,
    `const p = ${JSON.stringify(privateUnc)};`,
  ]
  const placeholders = [
    windowsDriveUserPath('example'),
    `const p = ${JSON.stringify(windowsDriveUserPath('username'))};`,
    windowsUncUserPath('runner'),
  ]

  for (const privatePath of privatePaths) {
    assert.ok(
      scanTextFindings('src/example.rs', privatePath)
        .some(finding => finding.code === 'personal-absolute-path'),
      `expected private path finding for ${privatePath}`,
    )
  }
  for (const placeholder of placeholders) {
    assert.equal(
      scanTextFindings('src/example.rs', placeholder)
        .some(finding => finding.code === 'personal-absolute-path'),
      false,
      `placeholder should be allowed: ${placeholder}`,
    )
  }
})

test('public package scripts cannot depend on excluded private modules', () => {
  const findings = auditPackageScripts('package.json', JSON.stringify({
    scripts: {
      build: 'vite build',
      'test:frontend-service': 'node --test src/frontend/services/bridgeFetch.test.ts',
      'test:private-ios': 'node --test ios-app/Tests/source.test.mjs',
      'test:private-extension': 'node --test browser-extension/background.test.mjs',
    },
  }))

  assert.deepEqual(findings.map(finding => finding.script), [
    'test:private-extension',
    'test:private-ios',
  ])
  assert.ok(findings.every(finding => finding.code === 'excluded-path-reference'))
})

test('public package scripts must reference files present in the export selection', () => {
  const findings = auditPackageScriptFiles('package.json', JSON.stringify({
    scripts: {
      good: 'node --test scripts/desktop-ok.test.mjs src/frontend/example.test.ts',
      missing: 'bash ./scripts/desktop-missing.sh',
    },
  }), [
    'scripts/desktop-ok.test.mjs',
    'src/frontend/example.test.ts',
  ])

  assert.deepEqual(findings, [{
    code: 'missing-script-file',
    path: 'scripts/desktop-missing.sh',
    script: 'missing',
  }])
})

test('public release and test transitive inputs are required export files', () => {
  const requiredInputs = [
    'LICENSE-UPSTREAM',
    'docs/INSTALLATION.md',
    'docs/INSTALL_PROMPT.md',
    'docs/SYSTEM_PROMPT.md',
    'docs/iterate_安装指南.md',
    'docs/release/INSTALLATION.md',
    'docs/release/INSTALL_PROMPT.md',
    'docs/verification-receipt-v0.md',
    'docs/worker-handoff-schema-v0.md',
    'scripts/desktop-bridge-web-push-security.test.mjs',
    'scripts/desktop-build-rs-target-gating.test.mjs',
    'scripts/desktop-community-activation.test.mjs',
    'scripts/desktop-codex-live-source.test.mjs',
    'scripts/desktop-ghost-suggestion-priority.test.mjs',
    'scripts/install-docs-contract.test.mjs',
    'scripts/desktop-speech-frontend-ownership.test.mjs',
  ]

  for (const requiredInput of requiredInputs) {
    assert.ok(
      manifest.requiredFiles.includes(requiredInput),
      `missing required export input: ${requiredInput}`,
    )
  }
})

test('current and upstream MIT notices are independently required', () => {
  assert.deepEqual(manifest.requiredLicenseNotices, [
    'Copyright (c) 2026 kexin94yyds',
  ])
  assert.deepEqual(manifest.requiredUpstreamLicenseNotices, [
    'Copyright (c) 2025 imshuo',
    'Based on cunzhi (https://github.com/imhuso/cunzhi)',
    'Copyright (c) 2024 imhuso',
  ])
  assert.deepEqual(manifest.requiredLicenseDigests, {
    LICENSE: '71cabcd1496fbb0a40166e0b55814e45ef509fd116e106c38dba81cd36a1f16d',
    'LICENSE-UPSTREAM': 'ac18099706256975b7f8de0e88506072618f2a05ef422f1974956040e31508ab',
  })
})

test('license digest gate rejects permission or warranty text changes', async () => {
  const upstream = await readFile(new URL('../LICENSE-UPSTREAM', import.meta.url))
  assert.deepEqual(
    auditLicenseDigest('LICENSE-UPSTREAM', upstream, manifest.requiredLicenseDigests),
    [],
  )

  const mutated = Buffer.from(
    upstream.toString('utf8').replace('Permission is hereby granted', 'Permission was removed'),
  )
  assert.deepEqual(
    auditLicenseDigest('LICENSE-UPSTREAM', mutated, manifest.requiredLicenseDigests),
    [{ code: 'license-digest-mismatch', path: 'LICENSE-UPSTREAM' }],
  )
})

test('desktop Tauri config does not retain private mobile signing configuration', async () => {
  const tauriConfig = JSON.parse(
    await readFile(new URL('../tauri.conf.json', import.meta.url), 'utf8'),
  )
  assert.equal(tauriConfig.bundle.iOS, undefined)
  assert.equal(JSON.stringify(tauriConfig).includes('developmentTeam'), false)
})

test('export destination must stay outside the real source worktree', async () => {
  const fixtureRoot = await mkdtemp(path.join(tmpdir(), 'iterate-oss-destination-'))
  const sourceRoot = path.join(fixtureRoot, 'source')
  const outsideRoot = path.join(fixtureRoot, 'outside')
  const sourceAlias = path.join(fixtureRoot, 'source-alias')
  await mkdir(sourceRoot)
  await mkdir(outsideRoot)
  await symlink(sourceRoot, sourceAlias, process.platform === 'win32' ? 'junction' : 'dir')

  try {
    assert.throws(
      () => validateExportDestination(sourceRoot, path.join(sourceRoot, 'export')),
      /outside the source worktree/,
    )
    assert.throws(
      () => validateExportDestination(sourceRoot, path.join(sourceAlias, 'export')),
      /outside the source worktree/,
    )
    assert.doesNotThrow(
      () => validateExportDestination(sourceRoot, path.join(outsideRoot, 'export')),
    )
  }
  finally {
    await rm(fixtureRoot, { recursive: true })
  }
})
