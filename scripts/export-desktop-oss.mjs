import { execFileSync } from 'node:child_process'
import { createHash } from 'node:crypto'
import {
  chmodSync,
  copyFileSync,
  existsSync,
  lstatSync,
  mkdirSync,
  readFileSync,
  realpathSync,
  writeFileSync,
} from 'node:fs'
import path from 'node:path'
import process from 'node:process'
import { fileURLToPath } from 'node:url'

const PRIVATE_SCRIPT_PATH_FRAGMENTS = [
  '.cunzhi-knowledge/',
  '.cunzhi-memory/',
  'artifacts/',
  'browser-extension/',
  'frontend-landing/',
  'gen/android/',
  'ios-app/',
  'server/',
  'services/',
  'vscode-extension',
]

const FORBIDDEN_AUDIO_EXTENSIONS = new Set(['.aac', '.flac', '.m4a', '.mp3', '.ogg', '.wav'])

const SENSITIVE_TEXT_PATTERNS = [
  {
    code: 'private-key-material',
    pattern: /-----BEGIN (?:RSA |EC |OPENSSH |DSA )?PRIVATE KEY-----/,
  },
  {
    code: 'github-token-material',
    pattern: /(?:^|[^A-Za-z0-9_])gh[pousr]_[A-Za-z0-9_]{20,}(?:$|[^A-Za-z0-9_])/,
  },
  {
    code: 'aws-access-key-material',
    pattern: /(?:^|[^A-Z0-9])AKIA[0-9A-Z]{16}(?:$|[^A-Z0-9])/,
  },
  {
    code: 'openai-key-material',
    pattern: /(?:^|[^A-Za-z0-9_-])sk-(?:proj-|svcacct-)?[A-Za-z0-9_-]{32,}(?:$|[^A-Za-z0-9_-])/,
  },
  {
    code: 'slack-token-material',
    pattern: /(?:^|[^A-Za-z0-9-])xox[baprs]-[A-Za-z0-9-]{10,}(?:$|[^A-Za-z0-9-])/,
  },
  {
    code: 'google-api-key-material',
    pattern: /(?:^|[^A-Za-z0-9_-])AIza[0-9A-Za-z_-]{30,}(?:$|[^A-Za-z0-9_-])/,
  },
  {
    code: 'credential-bearing-url',
    pattern: /https?:\/\/[A-Za-z0-9._~!$&()*+,;=%-]+:[^@\s/]{8,}@[A-Za-z0-9.-]+/,
  },
  {
    code: 'personal-absolute-path',
    pattern: /\/Users\/(?!example(?:\/|$)|username(?:\/|$)|runner(?:\/|$)|test(?:\/|$))[^/\s"']+/,
  },
  {
    code: 'personal-absolute-path',
    pattern: /\/home\/(?!example(?:\/|$)|username(?:\/|$)|runner(?:\/|$)|test(?:\/|$))[^/\s"']+/,
  },
  {
    code: 'personal-absolute-path',
    pattern: /\b[A-Za-z]:(?:\\{1,2}|\/)Users(?:\\{1,2}|\/)(?!(?:example|username|runner|test)(?:\\{1,2}|\/|[\s"'`]|$))[^\\/\s"']+/i,
  },
  {
    code: 'personal-absolute-path',
    pattern: /\\{2,4}[^\\/\s"']+(?:\\{1,2}|\/)Users(?:\\{1,2}|\/)(?!(?:example|username|runner|test)(?:\\{1,2}|\/|[\s"'`]|$))[^\\/\s"']+/i,
  },
]

function lineNumberAt(text, index) {
  return text.slice(0, index).split('\n').length
}

function normalizeRepositoryPath(filePath) {
  const normalized = filePath.replaceAll('\\', '/').replace(/^\.\//, '')
  if (!normalized || normalized.startsWith('/') || normalized.includes('\0'))
    throw new Error(`invalid repository path: ${filePath}`)
  if (normalized.split('/').some(segment => segment === '..'))
    throw new Error(`repository path escapes root: ${filePath}`)
  return normalized
}

function matchesPrefix(filePath, prefix) {
  return filePath.startsWith(prefix)
}

function pathIsExcluded(manifest, filePath) {
  return manifest.excludeFiles.includes(filePath)
    || manifest.excludePrefixes.some(prefix => matchesPrefix(filePath, prefix))
    || manifest.excludeSuffixes.some(suffix => filePath.toLowerCase().endsWith(suffix.toLowerCase()))
}

function pathIsIncluded(manifest, filePath) {
  const included = manifest.includeFiles.includes(filePath)
    || manifest.includePrefixes.some(prefix => matchesPrefix(filePath, prefix))
  return included && !pathIsExcluded(manifest, filePath)
}

export function selectIncludedPaths(manifest, trackedPaths) {
  return [...new Set(trackedPaths.map(normalizeRepositoryPath))]
    .filter(filePath => pathIsIncluded(manifest, filePath))
    .sort((left, right) => left.localeCompare(right))
}

export function scanTextFindings(filePath, text) {
  const findings = []
  for (const { code, pattern } of SENSITIVE_TEXT_PATTERNS) {
    const match = pattern.exec(text)
    if (!match)
      continue
    findings.push({
      code,
      path: filePath,
      line: lineNumberAt(text, match.index),
    })
  }
  return findings
}

export function auditWorkflowActionPins(filePath, text) {
  const findings = []
  for (const [index, line] of text.split('\n').entries()) {
    const match = line.match(/^\s*-?\s*uses:\s*['"]?([^'"\s#]+)['"]?/)
    if (!match)
      continue
    const action = match[1]
    if (action.startsWith('./'))
      continue
    if (!/@[0-9a-f]{40}$/i.test(action)) {
      findings.push({
        code: 'unpinned-action',
        path: filePath,
        line: index + 1,
      })
    }
  }
  return findings
}

export function auditPackageScripts(filePath, text, forbiddenFragments = PRIVATE_SCRIPT_PATH_FRAGMENTS) {
  let packageJson
  try {
    packageJson = JSON.parse(text)
  }
  catch {
    return [{ code: 'invalid-package-json', path: filePath }]
  }

  const findings = []
  for (const [script, command] of Object.entries(packageJson.scripts ?? {})) {
    if (typeof command !== 'string')
      continue
    const referencesPrivatePath = forbiddenFragments.some(fragment => {
      const escaped = fragment.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
      return new RegExp(`(^|[\\s"'=])(?:\\./)?${escaped}`).test(command)
    })
    if (referencesPrivatePath) {
      findings.push({
        code: 'excluded-path-reference',
        path: filePath,
        script,
      })
    }
  }
  return findings.sort((left, right) => left.script.localeCompare(right.script))
}

export function auditPackageScriptFiles(filePath, text, selectedPaths) {
  let packageJson
  try {
    packageJson = JSON.parse(text)
  }
  catch {
    return [{ code: 'invalid-package-json', path: filePath }]
  }

  const selected = new Set(selectedPaths)
  const findings = []
  const referencePattern = /(?:^|[\s"'=])((?:\.\/)?(?:scripts|src|tests)\/[^\s"'|;&]+?\.(?:bat|cjs|js|mjs|ps1|py|sh|ts|tsx))/g
  for (const [script, command] of Object.entries(packageJson.scripts ?? {})) {
    if (typeof command !== 'string')
      continue
    for (const match of command.matchAll(referencePattern)) {
      const referencedPath = normalizeRepositoryPath(match[1])
      if (!selected.has(referencedPath)) {
        findings.push({
          code: 'missing-script-file',
          path: referencedPath,
          script,
        })
      }
    }
  }
  return findings.sort((left, right) => {
    return `${left.script}:${left.path}`.localeCompare(`${right.script}:${right.path}`)
  })
}

export function auditSourceAssetPolicy(filePath) {
  if (filePath.startsWith('src/rust/assets/resources/')
    && FORBIDDEN_AUDIO_EXTENSIONS.has(path.extname(filePath).toLowerCase())) {
    return [{ code: 'audio-source-file-not-allowed', path: filePath }]
  }
  return []
}

export function auditLicenseDigest(filePath, bytes, expectedDigests) {
  const expected = expectedDigests?.[filePath]
  if (!expected)
    return []
  const canonicalBytes = Buffer.from(bytes.toString('utf8').replace(/\r\n/g, '\n'))
  const actual = createHash('sha256').update(canonicalBytes).digest('hex')
  return actual === expected
    ? []
    : [{ code: 'license-digest-mismatch', path: filePath }]
}

function gitOutput(sourceRoot, args) {
  return execFileSync('git', ['-C', sourceRoot, ...args], {
    encoding: 'utf8',
    maxBuffer: 128 * 1024 * 1024,
  }).trim()
}

function collectCandidateFiles(sourceRoot) {
  const output = execFileSync(
    'git',
    ['-C', sourceRoot, 'ls-files', '--cached', '--others', '--exclude-standard', '-z'],
    { maxBuffer: 128 * 1024 * 1024 },
  )
  return output.toString('utf8').split('\0').filter(Boolean)
}

function loadManifest(sourceRoot) {
  const manifestPath = path.join(sourceRoot, 'open-source-manifest.json')
  return JSON.parse(readFileSync(manifestPath, 'utf8'))
}

function binaryIsAllowed(manifest, filePath) {
  const extension = path.extname(filePath).toLowerCase()
  return manifest.binaryAllowExtensions.includes(extension)
    && manifest.binaryAllowPrefixes.some(prefix => matchesPrefix(filePath, prefix))
}

function auditSelectedFiles(sourceRoot, manifest, selectedPaths) {
  const findings = []

  for (const requiredFile of manifest.requiredFiles) {
    if (!selectedPaths.includes(requiredFile) || !existsSync(path.join(sourceRoot, requiredFile))) {
      findings.push({ code: 'missing-required-file', path: requiredFile })
    }
  }

  for (const filePath of selectedPaths) {
    const sourceAssetFindings = auditSourceAssetPolicy(filePath)
    if (sourceAssetFindings.length > 0) {
      findings.push(...sourceAssetFindings)
      continue
    }

    const absolutePath = path.resolve(sourceRoot, filePath)
    const relativePath = path.relative(sourceRoot, absolutePath)
    if (relativePath.startsWith('..') || path.isAbsolute(relativePath)) {
      findings.push({ code: 'path-escape', path: filePath })
      continue
    }

    const stat = lstatSync(absolutePath)
    if (stat.isSymbolicLink()) {
      findings.push({ code: 'symlink-not-allowed', path: filePath })
      continue
    }
    if (!stat.isFile()) {
      findings.push({ code: 'non-file-entry', path: filePath })
      continue
    }
    if (stat.size > manifest.maxFileBytes) {
      findings.push({ code: 'file-too-large', path: filePath })
      continue
    }

    const bytes = readFileSync(absolutePath)
    findings.push(...auditLicenseDigest(
      filePath,
      bytes,
      manifest.requiredLicenseDigests,
    ))
    const binary = bytes.subarray(0, Math.min(bytes.length, 8192)).includes(0)
    if (binary) {
      if (!binaryIsAllowed(manifest, filePath))
        findings.push({ code: 'binary-file-not-allowed', path: filePath })
      continue
    }

    const text = bytes.toString('utf8')
    findings.push(...scanTextFindings(filePath, text))
    if (filePath.startsWith('.github/workflows/'))
      findings.push(...auditWorkflowActionPins(filePath, text))
    if (filePath === 'package.json') {
      findings.push(...auditPackageScripts(
        filePath,
        text,
        manifest.forbiddenScriptPathFragments,
      ))
      findings.push(...auditPackageScriptFiles(filePath, text, selectedPaths))
    }
    if (filePath === 'LICENSE') {
      for (const notice of manifest.requiredLicenseNotices) {
        if (!text.includes(notice))
          findings.push({ code: 'missing-license-notice', path: filePath })
      }
    }
    if (filePath === 'LICENSE-UPSTREAM') {
      for (const notice of manifest.requiredUpstreamLicenseNotices ?? []) {
        if (!text.includes(notice))
          findings.push({ code: 'missing-upstream-license-notice', path: filePath })
      }
    }
  }

  return findings.sort((left, right) => {
    return `${left.code}:${left.path}:${left.script ?? ''}:${left.line ?? 0}`
      .localeCompare(`${right.code}:${right.path}:${right.script ?? ''}:${right.line ?? 0}`)
  })
}

function printFindings(findings) {
  const counts = new Map()
  for (const finding of findings)
    counts.set(finding.code, (counts.get(finding.code) ?? 0) + 1)

  console.error(`desktop OSS readiness failed: ${findings.length} finding(s)`)
  for (const [code, count] of [...counts.entries()].sort())
    console.error(`- ${code}: ${count}`)
  for (const finding of findings.slice(0, 80)) {
    const suffix = finding.script
      ? ` script=${finding.script}`
      : finding.line
        ? ` line=${finding.line}`
        : ''
    console.error(`  ${finding.code}: ${finding.path}${suffix}`)
  }
  if (findings.length > 80)
    console.error(`  ... ${findings.length - 80} more finding(s) omitted`)
}

function auditSource(sourceRoot) {
  const manifest = loadManifest(sourceRoot)
  const candidateFiles = collectCandidateFiles(sourceRoot)
  const selectedPaths = selectIncludedPaths(manifest, candidateFiles)
  const findings = auditSelectedFiles(sourceRoot, manifest, selectedPaths)
  try {
    execFileSync(
      'git',
      ['-C', sourceRoot, 'merge-base', '--is-ancestor', manifest.sourceBaseCommit, 'HEAD'],
      { stdio: 'ignore' },
    )
  }
  catch {
    findings.push({ code: 'source-base-mismatch', path: manifest.sourceBaseCommit })
  }
  return { manifest, selectedPaths, findings }
}

export function validateExportDestination(sourceRoot, destinationRoot) {
  const sourceRealPath = realpathSync.native(sourceRoot)
  const destinationPath = path.resolve(destinationRoot)
  const destinationParent = path.dirname(destinationPath)
  if (!existsSync(destinationParent))
    throw new Error('export destination parent must already exist')
  const destinationRealPath = path.join(
    realpathSync.native(destinationParent),
    path.basename(destinationPath),
  )
  const relative = path.relative(sourceRealPath, destinationRealPath)
  if (relative === '' || (!relative.startsWith('..') && !path.isAbsolute(relative)))
    throw new Error('export destination must be outside the source worktree')
}

function copySelectedFiles(sourceRoot, destinationRoot, selectedPaths) {
  validateExportDestination(sourceRoot, destinationRoot)
  if (existsSync(destinationRoot))
    throw new Error(`destination already exists: ${destinationRoot}`)
  mkdirSync(destinationRoot, { recursive: false })

  for (const filePath of selectedPaths) {
    const sourcePath = path.join(sourceRoot, filePath)
    const destinationPath = path.join(destinationRoot, filePath)
    mkdirSync(path.dirname(destinationPath), { recursive: true })
    copyFileSync(sourcePath, destinationPath)
    chmodSync(destinationPath, lstatSync(sourcePath).mode & 0o777)
  }
}

function manifestDigest(sourceRoot) {
  return createHash('sha256')
    .update(readFileSync(path.join(sourceRoot, 'open-source-manifest.json')))
    .digest('hex')
}

function isDirty(sourceRoot) {
  return gitOutput(sourceRoot, ['status', '--porcelain=v1']).length > 0
}

function runCheck(sourceRoot) {
  const { selectedPaths, findings } = auditSource(sourceRoot)
  if (findings.length > 0) {
    printFindings(findings)
    process.exitCode = 1
    return
  }
  console.log(`desktop OSS readiness passed: ${selectedPaths.length} file(s)`)
}

function runExport(sourceRoot, destinationRoot, allowDirty) {
  const { manifest, selectedPaths, findings } = auditSource(sourceRoot)
  if (findings.length > 0) {
    printFindings(findings)
    process.exitCode = 1
    return
  }

  const dirty = isDirty(sourceRoot)
  if (dirty && !allowDirty)
    throw new Error('source worktree is dirty; pass --allow-dirty only for local previews')

  copySelectedFiles(sourceRoot, destinationRoot, selectedPaths)
  const receipt = {
    schemaVersion: 1,
    project: manifest.project,
    distribution: manifest.distribution,
    sourceCommit: gitOutput(sourceRoot, ['rev-parse', 'HEAD']),
    dirty,
    manifestSha256: manifestDigest(sourceRoot),
    fileCount: selectedPaths.length,
  }
  writeFileSync(
    path.join(destinationRoot, 'SOURCE_RECEIPT.json'),
    `${JSON.stringify(receipt, null, 2)}\n`,
  )
  console.log(`desktop OSS snapshot exported: ${selectedPaths.length} file(s)`)
  console.log(`destination=${destinationRoot}`)
}

function main() {
  const [command = 'check', ...args] = process.argv.slice(2)
  if (command === 'check') {
    const sourceRoot = path.resolve(args[0] ?? '.')
    runCheck(sourceRoot)
    return
  }
  if (command === 'export') {
    const destination = args.find(argument => !argument.startsWith('--'))
    if (!destination)
      throw new Error('usage: export-desktop-oss.mjs export <destination> [--allow-dirty]')
    const sourceRoot = path.resolve('.')
    runExport(sourceRoot, path.resolve(destination), args.includes('--allow-dirty'))
    return
  }
  throw new Error(`unknown command: ${command}`)
}

const invokedPath = process.argv[1] ? path.resolve(process.argv[1]) : ''
if (invokedPath === fileURLToPath(import.meta.url)) {
  try {
    main()
  }
  catch (error) {
    console.error(error instanceof Error ? error.message : String(error))
    process.exitCode = 1
  }
}
