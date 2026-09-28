import { existsSync, readdirSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { spawnSync } from 'node:child_process'

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const cargoArgs = process.argv.slice(2)
if (cargoArgs[0] === '--')
  cargoArgs.shift()

function run(command, args, options = {}) {
  const result = spawnSync(command, args, {
    cwd: repoRoot,
    encoding: 'utf8',
    stdio: options.capture ? ['ignore', 'pipe', 'inherit'] : 'inherit',
    maxBuffer: 512 * 1024 * 1024,
  })

  if (result.error)
    throw result.error
  if (result.status !== 0)
    process.exit(result.status ?? 1)
  return result.stdout ?? ''
}

function findManifestTool() {
  const kitsRoot = join(process.env['ProgramFiles(x86)'] ?? 'C:\\Program Files (x86)', 'Windows Kits', '10', 'bin')
  if (!existsSync(kitsRoot))
    throw new Error('Windows SDK mt.exe was not found; install the Windows 10/11 SDK')

  const versions = readdirSync(kitsRoot, { withFileTypes: true })
    .filter(entry => entry.isDirectory())
    .map(entry => entry.name)
    .sort((left, right) => right.localeCompare(left, undefined, { numeric: true }))

  for (const version of versions) {
    for (const architecture of ['x64', 'x86']) {
      const candidate = join(kitsRoot, version, architecture, 'mt.exe')
      if (existsSync(candidate))
        return candidate
    }
  }

  throw new Error('Windows SDK mt.exe was not found; install the Windows 10/11 SDK')
}

if (process.platform !== 'win32') {
  run('cargo', ['test', '--locked', ...cargoArgs])
  process.exit(0)
}

const buildOutput = run('cargo', ['test', '--locked', '--no-run', '--message-format=json'], { capture: true })
const testExecutables = new Set()

for (const line of buildOutput.split(/\r?\n/)) {
  if (!line.startsWith('{'))
    continue

  const message = JSON.parse(line)
  if (message.reason === 'compiler-artifact' && message.profile?.test && message.executable)
    testExecutables.add(message.executable)
}

const mt = findManifestTool()
const manifest = join(repoRoot, 'resources', 'windows-test.manifest')
for (const executable of testExecutables)
  run(mt, ['-nologo', '-manifest', manifest, `-outputresource:${executable};#1`])

// Several existing suites share process-global registries and environment
// variables. Run them serially on Windows so the native executable runner
// matches Cargo's intended isolation instead of introducing cross-test races.
const executionArgs = cargoArgs.some(argument => argument.startsWith('--test-threads'))
  ? cargoArgs
  : [...cargoArgs, '--test-threads=1']

for (const executable of [...testExecutables].sort())
  run(executable, executionArgs)

if (cargoArgs.length === 0)
  run('cargo', ['test', '--locked', '--doc'])
