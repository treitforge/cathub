<#
.SYNOPSIS
    Build and start the standalone CatHub daemon.
#>
[CmdletBinding()]
param(
    [string]$Config,
    [switch]$DryRun,
    [switch]$DebugBuild
)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot

if (-not $Config) {
    if ($env:CATHUB_CONFIG_PATH) {
        $Config = $env:CATHUB_CONFIG_PATH
    }
    elseif ($IsWindows -or $env:OS -eq 'Windows_NT') {
        $Config = Join-Path $env:APPDATA 'cathub\cathub.toml'
    }
    elseif ($env:XDG_CONFIG_HOME) {
        $Config = Join-Path $env:XDG_CONFIG_HOME 'cathub/cathub.toml'
    }
    else {
        $Config = Join-Path $env:HOME '.config/cathub/cathub.toml'
    }
}

if (-not (Test-Path -LiteralPath $Config)) {
    $sample = Join-Path $repoRoot 'config\cathub.toml'
    Write-Warning "Config not found at $Config. Using the repository sample."
    $Config = $sample
}

$profile = if ($DebugBuild) { 'debug' } else { 'release' }
$binaryName = if ($IsWindows -or $env:OS -eq 'Windows_NT') { 'cathub.exe' } else { 'cathub' }
$binary = Join-Path $repoRoot "target\$profile\$binaryName"

if (-not (Test-Path -LiteralPath $binary)) {
    $buildArgs = @('build', '--workspace')
    if (-not $DebugBuild) { $buildArgs += '--release' }
    & cargo @buildArgs --manifest-path (Join-Path $repoRoot 'Cargo.toml')
    if ($LASTEXITCODE -ne 0) { throw "CatHub build failed with exit code $LASTEXITCODE" }
}

$runArgs = @('--config', $Config)
if ($DryRun) { $runArgs += '--dry-run' }
& $binary @runArgs
