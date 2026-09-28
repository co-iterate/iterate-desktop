param(
  [switch]$SkipTests
)

$ErrorActionPreference = "Stop"
$RepoRoot = Split-Path -Parent $PSScriptRoot
Push-Location $RepoRoot
try {
  $CargoBin = Join-Path $env:USERPROFILE ".cargo\bin"
  if (Test-Path -LiteralPath $CargoBin) {
    $env:PATH = "$CargoBin;$env:PATH"
  }
  $VsDevShell = "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\Common7\Tools\Launch-VsDevShell.ps1"
  if (Test-Path -LiteralPath $VsDevShell) {
    . $VsDevShell -Arch amd64 -HostArch amd64 -SkipAutomaticLocation
    $VsCMakeBin = "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin"
    if (Test-Path -LiteralPath $VsCMakeBin) {
      $env:PATH = "$VsCMakeBin;$env:PATH"
    }
  }
  $StrawberryPerlBin = "C:\Strawberry\perl\bin"
  if (Test-Path -LiteralPath $StrawberryPerlBin) {
    $env:PATH = "$StrawberryPerlBin;$env:PATH"
  }
  $StrawberryToolBin = "C:\Strawberry\c\bin"
  if (Test-Path -LiteralPath $StrawberryToolBin) {
    $env:PATH = "$StrawberryToolBin;$env:PATH"
  }
  if (-not (Get-Command nasm -ErrorAction SilentlyContinue)) {
    throw "NASM is required for the Windows release build. Install NASM or Strawberry Perl before building."
  }
  Remove-Item Env:AWS_LC_SYS_NO_ASM -ErrorAction SilentlyContinue
  $env:CARGO_PROFILE_RELEASE_OPT_LEVEL = "3"
  $env:CARGO_PROFILE_RELEASE_LTO = "thin"
  $env:CARGO_PROFILE_RELEASE_CODEGEN_UNITS = "16"

  if (-not $SkipTests) {
    corepack pnpm run lint:check
    if ($LASTEXITCODE -ne 0) { throw "Frontend lint failed" }
    corepack pnpm run test:rust
    if ($LASTEXITCODE -ne 0) { throw "Rust test suite failed" }
    corepack pnpm run test:windows-experience
    if ($LASTEXITCODE -ne 0) { throw "Windows experience contract tests failed" }
  }

  corepack pnpm exec tauri build --bundles nsis
  if ($LASTEXITCODE -ne 0) { throw "Windows NSIS build failed" }

  $Installer = Get-ChildItem -Path "target\release\bundle\nsis" -Filter "*-setup.exe" |
    Sort-Object LastWriteTime -Descending |
    Select-Object -First 1
  if (-not $Installer) { throw "NSIS installer was not produced" }

  Write-Host "Windows installer: $($Installer.FullName)"
}
finally {
  Pop-Location
}
