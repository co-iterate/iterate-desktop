#!/usr/bin/env node
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));

const checks = [
  {
    path: 'docs/loop-contract-v0.md',
    required: [
      'loop_id',
      'run_id',
      'timeline_route_id',
      'goal',
      'context_policy',
      'tool_policy',
      'verification',
      'handoff',
      'stop_conditions',
    ],
  },
  {
    path: 'docs/verification-receipt-v0.md',
    required: [
      'Goal',
      'Scope',
      'Changes',
      'Commands',
      'Evidence',
      'Risks',
      'Next Gate',
    ],
  },
  {
    path: 'docs/worker-handoff-schema-v0.md',
    required: [
      'Owner',
      'Task',
      'Inputs Reviewed',
      'Output',
      'Evidence',
      'Assumptions',
      'Risks',
      'Verification',
      'Open Questions',
      'Next Handoff',
      'pairoom',
      'built_in_subagent',
    ],
  },
];

function parseArgs(argv) {
  const options = {
    repoRoot: path.resolve(__dirname, '..'),
    json: false,
  };

  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === '--repo-root') {
      options.repoRoot = path.resolve(argv[i + 1] || '.');
      i += 1;
    } else if (arg === '--json') {
      options.json = true;
    } else {
      throw new Error(`Unknown argument: ${arg}`);
    }
  }

  return options;
}

function runChecks(repoRoot) {
  const failures = [];

  for (const check of checks) {
    const filePath = path.join(repoRoot, check.path);
    let contents = '';

    try {
      contents = fs.readFileSync(filePath, 'utf8');
    } catch (error) {
      failures.push({
        file: check.path,
        missing: ['<file>'],
        error: error.message,
      });
      continue;
    }

    const missing = check.required.filter(required => !contents.includes(required));
    if (missing.length > 0) {
      failures.push({ file: check.path, missing });
    }
  }

  return {
    status: failures.length === 0 ? 'ok' : 'failed',
    checked: checks.map(check => check.path),
    failures,
  };
}

function printText(result) {
  if (result.status === 'ok') {
    console.log(`loop contract check passed (${result.checked.length} files)`);
    return;
  }

  console.error('loop contract check failed');
  for (const failure of result.failures) {
    console.error(`- ${failure.file}: missing ${failure.missing.join(', ')}`);
    if (failure.error) {
      console.error(`  ${failure.error}`);
    }
  }
}

function main() {
  const options = parseArgs(process.argv.slice(2));
  const result = runChecks(options.repoRoot);

  if (options.json) {
    console.log(JSON.stringify(result, null, 2));
  } else {
    printText(result);
  }

  return result.status === 'ok' ? 0 : 1;
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  try {
    process.exitCode = main();
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}

export { runChecks };
