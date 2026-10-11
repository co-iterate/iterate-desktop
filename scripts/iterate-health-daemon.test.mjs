import assert from 'node:assert/strict'
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import os from 'node:os'
import path from 'node:path'
import { test } from 'node:test'

import {
  acquireLock,
  redactHealthResult,
  runDaemon,
  updateReceipt,
} from './iterate-health-daemon.mjs'

function sampleResult(collectedAt = '2026-06-28T08:00:00.000Z') {
  return {
    snapshot: {
      schema: 'iterate.health_loop.snapshot.v1',
      collectedAt,
      mode: 'read_only',
      ports: [
        {
          port: 5318,
          registeredWorkspace: '/Users/example/cunzhi',
          healthOk: true,
          statusOk: true,
          status: {
            is_busy: false,
            interaction_phase: 'idle',
            runtime: { pid: 12345 },
          },
        },
      ],
      processes: {
        counts: { serve: 2, mcpServers: 1, codexAppServers: 1 },
        serve: [{ pid: 12345, command: 'iterate --serve --port 5318 --workspace /Users/example/cunzhi' }],
      },
      connectionStatus: {
        websocket: {
          clients: [{ client_kind: 'ios', device_id: 'raw-device-id', auth_token: 'raw-token' }],
        },
      },
      connectionStatusSummary: {
        available: true,
        localOriginHealthy: true,
        publicTunnelHealthy: true,
        iosClientCount: 1,
      },
    },
    findings: [
      {
        code: 'workspace_idle_serve_over_budget',
        severity: 'warn',
        workspace: '/Users/example/cunzhi',
        idlePorts: [5314, 5318],
        message: '/Users/example/cunzhi has 2 idle serve ports',
      },
    ],
    recoveryPlan: {
      actions: [
        {
          code: 'trim_extra_idle_workspace_serve',
          mode: 'manual_confirm',
          exec: false,
          workspace: '/Users/example/cunzhi',
          reason: 'workspace_idle_serve_over_budget',
        },
      ],
    },
    recoveryResults: [
      {
        code: 'trim_extra_idle_workspace_serve',
        status: 'skipped',
        workspace: '/Users/example/cunzhi',
        reason: 'exec_false',
      },
    ],
  }
}

test('redactHealthResult removes raw connection payloads and keeps operational summary', () => {
  const record = redactHealthResult(sampleResult())

  assert.equal(record.schema, 'iterate.health_loop.daemon_record.v1')
  assert.equal(record.snapshot.connectionStatus, undefined)
  assert.equal(record.snapshot.processes, undefined)
  assert.deepEqual(record.snapshot.processCounts, {
    serve: 2,
    mcpServers: 1,
    codexAppServers: 1,
  })
  assert.deepEqual(record.snapshot.ports, [
    {
      port: 5318,
      registeredWorkspace: '/Users/example/cunzhi',
      healthOk: true,
      statusOk: true,
      isBusy: false,
      interactionPhase: 'idle',
    },
  ])
  assert.equal(JSON.stringify(record).includes('raw-device-id'), false)
  assert.equal(JSON.stringify(record).includes('raw-token'), false)
})

test('updateReceipt aggregates finding counts and last recovery state', () => {
  const first = redactHealthResult(sampleResult('2026-06-28T08:00:00.000Z'))
  const second = redactHealthResult(sampleResult('2026-06-28T08:01:00.000Z'))

  const receipt = updateReceipt(updateReceipt(null, first), second)

  assert.equal(receipt.schema, 'iterate.health_loop.receipt.v1')
  assert.equal(receipt.findings.length, 1)
  assert.equal(receipt.findings[0].count, 2)
  assert.equal(receipt.findings[0].firstSeenAt, '2026-06-28T08:00:00.000Z')
  assert.equal(receipt.findings[0].lastSeenAt, '2026-06-28T08:01:00.000Z')
  assert.equal(receipt.findings[0].lastAction.code, 'trim_extra_idle_workspace_serve')
  assert.equal(receipt.findings[0].lastResult.status, 'skipped')
})

test('updateReceipt associates repeated finding codes with matching recovery state', () => {
  const record = {
    collectedAt: '2026-06-28T08:00:00.000Z',
    findings: [
      {
        code: 'dead_port_registration',
        severity: 'warn',
        port: 5314,
        message: 'registered port 5314 does not answer /health',
      },
      {
        code: 'dead_port_registration',
        severity: 'warn',
        port: 5318,
        message: 'registered port 5318 does not answer /health',
      },
    ],
    recoveryPlan: {
      actions: [
        {
          code: 'prune_dead_port_registration',
          mode: 'auto',
          exec: true,
          port: 5314,
          reason: 'dead_port_registration',
        },
        {
          code: 'prune_dead_port_registration',
          mode: 'auto',
          exec: true,
          port: 5318,
          reason: 'dead_port_registration',
        },
      ],
    },
    recoveryResults: [
      {
        code: 'prune_dead_port_registration',
        status: 'applied',
        port: 5314,
      },
      {
        code: 'prune_dead_port_registration',
        status: 'applied',
        port: 5318,
      },
    ],
  }

  const receipt = updateReceipt(null, record)

  assert.deepEqual(
    receipt.findings.map(finding => ({
      port: finding.port,
      actionPort: finding.lastAction?.port,
      resultPort: finding.lastResult?.port,
    })),
    [
      { port: 5314, actionPort: 5314, resultPort: 5314 },
      { port: 5318, actionPort: 5318, resultPort: 5318 },
    ],
  )
})

test('acquireLock rejects a live duplicate and replaces a stale lock', async () => {
  const tempDir = await mkdtemp(path.join(os.tmpdir(), 'iterate-health-daemon-lock-'))
  const lockPath = path.join(tempDir, 'daemon.lock')

  const lock = await acquireLock(lockPath, { pid: 111, isProcessAlive: () => true })
  await assert.rejects(
    () => acquireLock(lockPath, { pid: 222, isProcessAlive: () => true }),
    /already running/,
  )
  await lock.release()

  await writeFile(lockPath, JSON.stringify({ pid: 333, startedAt: 'stale' }), 'utf8')
  const replacement = await acquireLock(lockPath, { pid: 444, isProcessAlive: () => false })
  await replacement.release()

  await rm(tempDir, { recursive: true, force: true })
})

test('runDaemon creates parent directories for custom records and receipt paths', async () => {
  const tempDir = await mkdtemp(path.join(os.tmpdir(), 'iterate-health-daemon-custom-'))
  const recordsPath = path.join(tempDir, 'custom-records', 'records.jsonl')
  const receiptPath = path.join(tempDir, 'custom-receipts', 'receipt.json')

  await runDaemon({
    runDir: path.join(tempDir, 'run'),
    recordsPath,
    receiptPath,
    once: true,
    collect: async () => sampleResult(),
  })

  assert.match(await readFile(recordsPath, 'utf8'), /iterate.health_loop.daemon_record.v1/)
  const receipt = JSON.parse(await readFile(receiptPath, 'utf8'))
  assert.equal(receipt.schema, 'iterate.health_loop.receipt.v1')

  await rm(tempDir, { recursive: true, force: true })
})

test('runDaemon writes redacted JSONL records and a compact receipt', async () => {
  const tempDir = await mkdtemp(path.join(os.tmpdir(), 'iterate-health-daemon-run-'))
  let calls = 0

  await runDaemon({
    runDir: tempDir,
    lockFile: path.join(tempDir, 'daemon.lock'),
    maxIterations: 2,
    intervalSecs: 1,
    collect: async () => {
      calls += 1
      return sampleResult(`2026-06-28T08:0${calls}:00.000Z`)
    },
    sleep: async () => {},
  })

  const jsonl = await readFile(path.join(tempDir, 'records.jsonl'), 'utf8')
  const lines = jsonl.trim().split('\n')
  assert.equal(lines.length, 2)
  assert.equal(jsonl.includes('raw-device-id'), false)

  const receipt = JSON.parse(await readFile(path.join(tempDir, 'receipt.json'), 'utf8'))
  assert.equal(receipt.findings[0].count, 2)
  assert.equal(receipt.findings[0].lastSeenAt, '2026-06-28T08:02:00.000Z')

  await rm(tempDir, { recursive: true, force: true })
})
