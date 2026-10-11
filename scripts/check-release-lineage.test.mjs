import assert from 'node:assert/strict'
import { execFileSync, spawnSync } from 'node:child_process'
import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import path from 'node:path'
import test from 'node:test'

const script = new URL('check-release-lineage.sh', import.meta.url).pathname

function git(cwd, ...args) {
  return execFileSync('git', args, { cwd, encoding: 'utf8' }).trim()
}

function fixture() {
  const root = mkdtempSync(path.join(tmpdir(), 'release-lineage-'))
  const repo = path.join(root, 'repo')
  mkdirSync(repo)
  git(repo, 'init', '-q', '-b', 'release/desktop')
  git(repo, 'config', 'user.email', 't@example.invalid')
  git(repo, 'config', 'user.name', 't')
  const commit = (name) => {
    writeFileSync(path.join(repo, name), name)
    git(repo, 'add', name)
    git(repo, 'commit', '-qm', name)
    return git(repo, 'rev-parse', 'HEAD')
  }
  const base = commit('base')
  const installed = commit('installed-fix')
  git(repo, 'checkout', '-q', '-b', 'side', base)
  const sideOnly = commit('side-only')
  git(repo, 'checkout', '-q', 'release/desktop')
  git(repo, 'merge', '-q', '--no-ff', '-m', 'merge side', 'side')
  const merged = git(repo, 'rev-parse', 'HEAD')
  const app = (name, sourceCommit) => {
    const resources = path.join(root, `${name}.app`, 'Contents', 'Resources')
    mkdirSync(resources, { recursive: true })
    writeFileSync(path.join(resources, 'cunzhi-source-receipt.json'), JSON.stringify({ source_commit: sourceCommit }))
    return path.join(root, `${name}.app`)
  }
  return { root, repo, base, installed, sideOnly, merged, app, cleanup: () => rmSync(root, { recursive: true, force: true }) }
}

function run(f, source, installed, env = {}) {
  return spawnSync('bash', [script, f.repo, source, installed], { encoding: 'utf8', env: { ...process.env, ...env } })
}

test('accepts a source that contains the mainline and the installed commit', () => {
  const f = fixture()
  try {
    const result = run(f, f.app('src', f.merged), f.app('inst', f.installed))
    assert.equal(result.status, 0, result.stderr)
    assert.match(result.stdout, /lineage: ok/)
  } finally { f.cleanup() }
})

test('rejects a side branch build that would drop the mainline fix', () => {
  const f = fixture()
  try {
    const result = run(f, f.app('src', f.sideOnly), f.app('inst', f.base))
    assert.equal(result.status, 3)
    assert.match(result.stderr, /不包含 release\/desktop 最新提交/)
  } finally { f.cleanup() }
})

test('rejects overwriting the installed commit even when the mainline check is skipped', () => {
  const f = fixture()
  try {
    const result = run(f, f.app('src', f.sideOnly), f.app('inst', f.installed), { CUNZHI_RELEASE_BRANCH: 'no-such-branch' })
    assert.equal(result.status, 3)
    assert.match(result.stderr, /不包含已安装的/)
  } finally { f.cleanup() }
})

test('explicit override continues with a warning', () => {
  const f = fixture()
  try {
    const result = run(f, f.app('src', f.base), f.app('inst', f.installed), { CUNZHI_MACOS_ALLOW_LINEAGE_REGRESSION: '1' })
    assert.equal(result.status, 0)
    assert.match(result.stderr, /按用户明确要求继续安装/)
  } finally { f.cleanup() }
})

test('missing installed receipt only skips the installed check', () => {
  const f = fixture()
  try {
    const result = run(f, f.app('src', f.merged), path.join(f.root, 'none.app'))
    assert.equal(result.status, 0, result.stderr)
    assert.match(result.stderr, /跳过已安装检查/)
  } finally { f.cleanup() }
})

test('rejects a source missing commits that only the remote mainline has', () => {
  const f = fixture()
  try {
    // 本地 release/desktop 停在 merged，远端已经多了一个提交（push HEAD:release/desktop 不会更新本地分支）
    writeFileSync(path.join(f.repo, 'remote-only'), 'remote-only')
    git(f.repo, 'add', 'remote-only')
    git(f.repo, 'commit', '-qm', 'remote-only')
    const remoteTip = git(f.repo, 'rev-parse', 'HEAD')
    git(f.repo, 'update-ref', 'refs/remotes/origin/release/desktop', remoteTip)
    git(f.repo, 'reset', '-q', '--hard', f.merged)
    const result = run(f, f.app('src', f.merged), f.app('inst', f.installed))
    assert.equal(result.status, 3)
    assert.match(result.stderr, /不包含 origin\/release\/desktop 最新提交/)
  } finally { f.cleanup() }
})
