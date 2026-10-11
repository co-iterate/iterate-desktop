import assert from 'node:assert/strict'
import { mkdtemp, readFile, rm, stat, writeFile } from 'node:fs/promises'
import os from 'node:os'
import path from 'node:path'
import { test } from 'node:test'

import {
  applyRecoveryPlan,
  classifySnapshot,
  createRecoveryPlan,
  summarizeConnectionStatus,
} from './iterate-health-loop.mjs'

test('classifySnapshot reports dead registrations and excess idle serve processes', () => {
  const snapshot = {
    ports: [
      {
        port: 5314,
        registeredWorkspace: '/Users/example/cunzhi',
        healthOk: true,
        statusOk: true,
        status: { is_busy: false, interaction_phase: 'idle', runtime: { pid: 1001 } },
      },
      {
        port: 5317,
        registeredWorkspace: '/Users/example/cunzhi',
        healthOk: true,
        statusOk: true,
        status: { is_busy: false, interaction_phase: 'idle', runtime: { pid: 1002 } },
      },
      {
        port: 5318,
        registeredWorkspace: '/Users/example/cunzhi',
        healthOk: false,
        statusOk: false,
        error: 'connect ECONNREFUSED',
      },
    ],
    connectionStatus: null,
  }

  const findings = classifySnapshot(snapshot, { maxIdleServePerWorkspace: 1 })

  assert.deepEqual(
    findings.map(finding => finding.code),
    ['dead_port_registration', 'workspace_idle_serve_over_budget'],
  )
  assert.equal(findings[0].severity, 'warn')
  assert.equal(findings[0].port, 5318)
  assert.equal(findings[1].workspace, '/Users/example/cunzhi')
  assert.equal(findings[1].idlePorts.length, 2)
})

test('classifySnapshot reports stale busy and active workspace mismatches', () => {
  const oldPhase = new Date(Date.now() - 10 * 60 * 1000).toISOString()
  const snapshot = {
    ports: [
      {
        port: 5320,
        registeredWorkspace: '/Users/example/cunzhi',
        healthOk: true,
        statusOk: true,
        status: {
          is_busy: true,
          interaction_phase: 'starting_gui',
          phase_since: oldPhase,
          active_request_id: 'req-1',
          active_workspace: '/Users/example/other',
          runtime: { pid: 2001 },
        },
      },
    ],
    connectionStatus: null,
  }

  const findings = classifySnapshot(snapshot, {
    staleBusyTransientSecs: 30,
    maxIdleServePerWorkspace: 1,
  })

  assert.deepEqual(
    findings.map(finding => finding.code),
    ['active_workspace_mismatch', 'stale_busy_port'],
  )
  assert.equal(findings[0].port, 5320)
  assert.equal(findings[1].phase, 'starting_gui')
})

test('classifySnapshot treats sibling workspace path prefixes as mismatches', () => {
  const snapshot = {
    ports: [
      {
        port: 5320,
        registeredWorkspace: '/Users/example/cunzhi',
        healthOk: true,
        statusOk: true,
        status: {
          is_busy: true,
          active_request_id: 'req-1',
          interaction_phase: 'waiting_user',
          active_workspace: '/Users/example/cunzhi-other',
        },
      },
    ],
    connectionStatus: null,
  }

  const findings = classifySnapshot(snapshot)

  assert.deepEqual(
    findings.map(finding => finding.code),
    ['active_workspace_mismatch'],
  )
  assert.equal(findings[0].activeWorkspace, '/Users/example/cunzhi-other')
})

test('summarizeConnectionStatus keeps public auth protection separate from failure', () => {
  const summary = summarizeConnectionStatus({
    diagnosis: { code: 'ok' },
    local_origin: { healthy: true },
    public_tunnel: {
      healthy: true,
      websocket_auth_required: true,
      websocket: { upgrade_ok: false },
    },
    root_tunnel: {
      derived: {
        tunnel_health_class: 'healthy',
        backoff_active: false,
        backoff_remaining_secs: 0,
      },
      metrics: { effective_ha_connection_count: 4 },
    },
    websocket: {
      client_count: 1,
      clients: [
        {
          client_kind: 'ios',
          selected_transport_mode: 'public_tunnel',
          last_seen_at: new Date(Date.now() - 20_000).toISOString(),
          last_message_type: 'request_sync',
        },
      ],
    },
  })

  assert.equal(summary.localOriginHealthy, true)
  assert.equal(summary.publicTunnelHealthy, true)
  assert.equal(summary.publicWebSocketAuthRequired, true)
  assert.equal(summary.publicWebSocketProtectedHealthy, true)
  assert.equal(summary.iosClientCount, 1)
  assert.equal(summary.iosPrimaryMode, 'public_tunnel')
  assert.ok(summary.iosLastSeenAgeSecs >= 0)
})

test('createRecoveryPlan is dry-run and never emits destructive actions by default', () => {
  const findings = [
    { code: 'dead_port_registration', severity: 'warn', port: 5318 },
    { code: 'workspace_idle_serve_over_budget', severity: 'warn', workspace: '/Users/example/cunzhi' },
    { code: 'public_tunnel_unhealthy', severity: 'warn' },
  ]

  const plan = createRecoveryPlan(findings)

  assert.deepEqual(
    plan.actions.map(action => action.mode),
    ['dry_run', 'manual_confirm', 'dry_run'],
  )
  assert.equal(plan.destructiveActionsEnabled, false)
  assert.ok(plan.actions.every(action => action.exec === false))
})

test('createRecoveryPlan enables only safe P2 actions when requested', () => {
  const findings = [
    { code: 'dead_port_registration', severity: 'warn', port: 5318 },
    { code: 'workspace_idle_serve_over_budget', severity: 'warn', workspace: '/Users/example/cunzhi' },
    { code: 'public_tunnel_unhealthy', severity: 'warn' },
  ]

  const plan = createRecoveryPlan(findings, { safeRecoveryEnabled: true })

  assert.equal(plan.safeRecoveryEnabled, true)
  assert.deepEqual(
    plan.actions.map(action => ({ code: action.code, mode: action.mode, exec: action.exec })),
    [
      { code: 'prune_dead_port_registration', mode: 'auto', exec: true },
      { code: 'trim_extra_idle_workspace_serve', mode: 'manual_confirm', exec: false },
      { code: 'request_public_tunnel_recovery', mode: 'auto', exec: true },
    ],
  )
})

test('applyRecoveryPlan executes only explicit safe recovery actions', async () => {
  const tempDir = await mkdtemp(path.join(os.tmpdir(), 'iterate-health-loop-'))
  const recoverRequestFile = path.join(tempDir, 'recover.request')
  const portFile = path.join(tempDir, '5318')
  await writeFile(portFile, '/Users/example/cunzhi\n')

  const plan = {
    actions: [
      { code: 'prune_dead_port_registration', mode: 'auto', exec: true, port: 5318 },
      { code: 'request_public_tunnel_recovery', mode: 'auto', exec: true, reason: 'public_tunnel_unhealthy' },
      { code: 'trim_extra_idle_workspace_serve', mode: 'manual_confirm', exec: false, workspace: '/Users/example/cunzhi' },
    ],
  }

  const results = await applyRecoveryPlan(plan, { portDir: tempDir, recoverRequestFile })

  await assert.rejects(stat(portFile), { code: 'ENOENT' })
  const recoverBody = await readFile(recoverRequestFile, 'utf8')
  assert.match(recoverBody, /iterate_health_loop_recover=public_tunnel_unhealthy/)
  assert.deepEqual(
    results.map(result => ({ code: result.code, status: result.status })),
    [
      { code: 'prune_dead_port_registration', status: 'applied' },
      { code: 'request_public_tunnel_recovery', status: 'applied' },
      { code: 'trim_extra_idle_workspace_serve', status: 'skipped' },
    ],
  )

  await rm(tempDir, { recursive: true, force: true })
})
