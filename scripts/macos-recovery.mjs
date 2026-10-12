#!/usr/bin/env node
import { execFileSync } from 'node:child_process'
import { createHash, randomUUID } from 'node:crypto'
import { existsSync, lstatSync, mkdirSync, readdirSync, readFileSync, readlinkSync, realpathSync, renameSync, rmSync, writeFileSync } from 'node:fs'
import { homedir } from 'node:os'
import path from 'node:path'
import process from 'node:process'
import { fileURLToPath } from 'node:url'

const binary = app => path.join(app, 'Contents/MacOS/iterate')
const hash = bytes => createHash('sha256').update(bytes).digest('hex')
const run = (command, args) => execFileSync(command, args, { encoding: 'utf8', timeout: 10000, stdio: ['ignore', 'pipe', 'pipe'] })

function directory(app) {
  if (!lstatSync(app).isDirectory() || lstatSync(app).isSymbolicLink())
    throw new Error(`Expected a real app directory: ${app}`)
}

function signature(app) {
  directory(app)
  run('codesign', ['--verify', '--deep', '--strict', app])
}

export function bundleHash(app) {
  const entries = []
  function visit(relative) {
    const absolute = path.join(app, relative)
    const info = lstatSync(absolute)
    if (info.isSymbolicLink()) {
      entries.push([relative, 'link', readlinkSync(absolute)])
    }
    else if (info.isDirectory()) {
      for (const name of readdirSync(absolute).sort()) visit(path.join(relative, name))
    }
    else if (info.isFile()) {
      entries.push([relative, 'file', info.mode & 0o777, hash(readFileSync(absolute))])
    }
    else {
      throw new Error(`Unsupported bundle entry: ${absolute}`)
    }
  }
  visit('')
  return hash(JSON.stringify(entries))
}

// No build, package manager, source checkout, process termination or data reset.
export function backupApp(app, output) {
  app = path.resolve(app)
  output = path.resolve(output)
  if (existsSync(output))
    throw new Error(`Backup already exists: ${output}`)
  signature(app)
  const expected = bundleHash(app)
  const temporary = `${output}.pending-${randomUUID()}`
  const copy = path.join(temporary, 'iterate.app')
  mkdirSync(temporary, { recursive: true })
  try {
    run('ditto', [app, copy])
    signature(copy)
    if (bundleHash(copy) !== expected || bundleHash(app) !== expected)
      throw new Error('Backup differs from the installed bundle; installation must stop')
    const receipt = { schema_version: 1, bundle: 'iterate.app', bundle_sha256: expected, executable_sha256: hash(readFileSync(binary(copy))), original_app: app, created_at: new Date().toISOString() }
    writeFileSync(path.join(temporary, 'receipt.json'), `${JSON.stringify(receipt, null, 2)}\n`)
    renameSync(temporary, output)
    return { status: 'backup-verified', receipt: path.join(output, 'receipt.json') }
  }
  catch (error) {
    rmSync(temporary, { recursive: true, force: true })
    throw error
  }
}

export function restoreApp(receiptPath, app) {
  receiptPath = path.resolve(receiptPath)
  app = path.resolve(app)
  const receipt = JSON.parse(readFileSync(receiptPath, 'utf8'))
  if (receipt.schema_version !== 1 || receipt.bundle !== 'iterate.app')
    throw new Error('Unsupported backup receipt')
  const backup = path.join(path.dirname(receiptPath), receipt.bundle)
  if (app === backup || backup.startsWith(`${app}${path.sep}`) || app.startsWith(`${backup}${path.sep}`))
    throw new Error('Restore and backup paths overlap')
  signature(backup)
  if (bundleHash(backup) !== receipt.bundle_sha256 || hash(readFileSync(binary(backup))) !== receipt.executable_sha256)
    throw new Error('Backup hash mismatch; current installation was not changed')
  if (existsSync(app))
    directory(app)
  const nonce = randomUUID()
  const staging = path.join(path.dirname(app), `.iterate-restore-${nonce}.app`)
  const retired = path.join(path.dirname(app), '.iterate-retired', `iterate-${nonce}.app`)
  let moved = false
  try {
    run('ditto', [backup, staging])
    signature(staging)
    if (bundleHash(staging) !== receipt.bundle_sha256)
      throw new Error('Staged restore hash mismatch')
    if (existsSync(app)) {
      mkdirSync(path.dirname(retired), { recursive: true })
      renameSync(app, retired)
      moved = true
    }
    try {
      renameSync(staging, app)
    }
    catch (error) {
      if (moved)
        renameSync(retired, app)
      throw error
    }
    return { status: 'bundle-restored', app, retired: moved ? retired : null, runtime_refresh_required: true, foreground_and_background_processes_preserved: true }
  }
  finally { rmSync(staging, { recursive: true, force: true }) }
}

export function verifyProduction(app) {
  const executable = binary(path.resolve(app))
  const before = hash(readFileSync(executable))
  // Older binaries safely handle --version; their non-JSON response is rejected.
  const info = JSON.parse(run(executable, ['--desktop-build-info', '--version']))
  if (info.schema_version !== 1 || info.production !== true || info.ui_readiness_schema !== 1)
    throw new Error('Candidate is not a verifiable Tauri production build')
  run(executable, ['--check-frontend-assets'])
  if (hash(readFileSync(executable)) !== before)
    throw new Error('Candidate changed during verification')
  return { status: 'production-verified', executable_sha256: before }
}

export function validateReadiness(receipt, { pid, since, executable, sha256, now = Date.now() }) {
  if (receipt.schema_version !== 1 || receipt.pid !== pid || receipt.production !== true
    || receipt.role !== 'canonical-gui' || receipt.window !== 'main' || receipt.ready !== true
    || !['application', 'activation'].includes(receipt.surface)
    || !['tauri://localhost', 'https://tauri.localhost'].includes(receipt.origin)
    || !Number.isFinite(receipt.ready_at_ms) || receipt.ready_at_ms < since || receipt.ready_at_ms > now + 5000
    || receipt.executable_sha256 !== sha256 || receipt.executable !== executable) {
    throw new Error('Fresh production main-window readiness has not been established')
  }
}

export async function waitForReady(app, pid, since, timeoutMs = 30000) {
  if (!Number.isSafeInteger(pid) || pid <= 0 || !Number.isFinite(since) || since <= 0)
    throw new Error('A specific GUI PID and launch timestamp are required')
  const executable = realpathSync(binary(path.resolve(app)))
  const sha256 = hash(readFileSync(executable))
  const receiptPath = path.join(homedir(), '.cunzhi/desktop-health', `${pid}.json`)
  const deadline = Date.now() + timeoutMs
  let lastError
  do {
    try {
      process.kill(pid, 0)
      const receipt = JSON.parse(readFileSync(receiptPath, 'utf8'))
      validateReadiness(receipt, { pid, since, executable, sha256 })
      const mappings = run('lsof', ['-a', '-p', String(pid), '-d', 'txt', '-Fn'])
      if (!mappings.split('\n').includes(`n${executable}`))
        throw new Error('GUI maps a different installed bundle')
      return { status: 'ui-ready', pid, receipt: receiptPath, executable_sha256: sha256 }
    }
    catch (error) { lastError = error }
    await new Promise(resolve => setTimeout(resolve, 250))
  } while (Date.now() < deadline)
  throw new Error(`GUI readiness timeout: ${lastError?.message}`)
}

async function main() {
  const [command, ...args] = process.argv.slice(2)
  const options = {}
  for (let i = 0; i < args.length; i += 2) {
    if (!args[i].startsWith('--') || !args[i + 1])
      throw new Error('Expected named arguments with values')
    options[args[i].slice(2)] = args[i + 1]
  }
  if (!options.app)
    throw new Error('--app is required')
  let result
  if (command === 'backup' && options.out)
    result = backupApp(options.app, options.out)
  else if (command === 'restore' && options.receipt)
    result = restoreApp(options.receipt, options.app)
  else if (command === 'production')
    result = verifyProduction(options.app)
  else if (command === 'wait-ready')
    result = await waitForReady(options.app, Number(options.pid), Number(options.since), Number(options.timeout ?? 30000))
  else throw new Error('Use backup --out, restore --receipt, production, or wait-ready --pid --since')
  process.stdout.write(`${JSON.stringify(result)}\n`)
}

if (process.argv[1] && realpathSync(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().catch((error) => {
    process.stderr.write(`${error.message}\n`)
    process.exitCode = 1
  })
}
