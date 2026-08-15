[CmdletBinding()]
param(
    [ValidateSet('Check', 'ValidatePackage', 'Package')]
    [string]$Action = 'Check'
)

$ErrorActionPreference = 'Stop'
$driverRoot = Join-Path $PSScriptRoot '..\drivers\cathub-virtual-serial-umdf'

function Assert-Command {
    param([Parameter(Mandatory)][string]$Name)

    if (-not (Get-Command $Name -ErrorAction SilentlyContinue)) {
        throw "Required UMDF build command '$Name' is not available. Use a supported WDK developer prompt."
    }
}

function Test-WdkContentRoot {
    param([Parameter(Mandatory)][string]$Path)

    if (-not (Test-Path -LiteralPath (Join-Path $Path 'Include'))) {
        return $false
    }

    return @(Get-ChildItem -LiteralPath (Join-Path $Path 'Include') -Directory |
        Where-Object { Test-Path -LiteralPath (Join-Path $_.FullName 'km\crt') }).Count -gt 0
}

function Find-WdkContentRoot {
    if ($env:WDKContentRoot -and (Test-WdkContentRoot $env:WDKContentRoot)) {
        return (Resolve-Path -LiteralPath $env:WDKContentRoot).Path
    }

    $packageRoot = Join-Path $env:LOCALAPPDATA 'CatHub\wdk\packages'
    if (Test-Path -LiteralPath $packageRoot) {
        $packagePrefix = 'Microsoft.Windows.WDK.x64.'
        $packages = @(Get-ChildItem -LiteralPath $packageRoot -Directory |
            Where-Object {
                $_.Name.StartsWith($packagePrefix) -and (Test-WdkContentRoot (Join-Path $_.FullName 'c'))
            } |
            Sort-Object { [version]$_.Name.Substring($packagePrefix.Length) } -Descending)
        if ($packages.Count -gt 0) {
            return (Join-Path $packages[0].FullName 'c')
        }
    }

    $kitsRoot = (Get-ItemProperty `
        -LiteralPath 'HKLM:\SOFTWARE\Microsoft\Windows Kits\Installed Roots' `
        -ErrorAction SilentlyContinue).KitsRoot10
    if ($kitsRoot -and (Test-WdkContentRoot $kitsRoot)) {
        return $kitsRoot
    }

    throw @"
No complete WDK was found. Install the pinned user-local package with:
nuget install Microsoft.Windows.WDK.x64 -Version 10.0.28000.2526 -OutputDirectory `"$packageRoot`" -NonInteractive -DirectDownload -Source https://api.nuget.org/v3/index.json
"@
}

function Add-PathEntry {
    param([Parameter(Mandatory)][string]$Path)

    if ((Test-Path -LiteralPath $Path) -and ($env:Path -split ';' -notcontains $Path)) {
        $env:Path = "$Path;$env:Path"
    }
}

function Initialize-WdkEnvironment {
    $contentRoot = Find-WdkContentRoot
    $versionDirectory = Get-ChildItem -LiteralPath (Join-Path $contentRoot 'Include') -Directory |
        Where-Object { Test-Path -LiteralPath (Join-Path $_.FullName 'km\crt') } |
        Sort-Object { [version]$_.Name } -Descending |
        Select-Object -First 1

    $env:WDKContentRoot = $contentRoot
    $binRoot = Join-Path $contentRoot "bin\$($versionDirectory.Name)"
    $toolRoot = Join-Path $contentRoot "tools\$($versionDirectory.Name)"
    if (Test-Path -LiteralPath $binRoot) {
        $env:WDKBinRoot = $binRoot
        Add-PathEntry (Join-Path $binRoot 'x86')
        Add-PathEntry (Join-Path $binRoot 'x64')
    }
    if (Test-Path -LiteralPath $toolRoot) {
        $env:WDKToolRoot = $toolRoot
        Add-PathEntry (Join-Path $toolRoot 'x64')
    }

    Write-Host "Using WDK $($versionDirectory.Name) from '$contentRoot'."
}

function Invoke-Checked {
    param(
        [Parameter(Mandatory)][string]$Command,
        [Parameter(Mandatory)][string[]]$Arguments
    )

    & $Command @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "'$Command $($Arguments -join ' ')' failed with exit code $LASTEXITCODE."
    }
}

Assert-Command cargo
Assert-Command clang
Initialize-WdkEnvironment

Push-Location $driverRoot
try {
    if ($Action -eq 'Package') {
        foreach ($command in @(
                'cargo-make', 'inf2cat', 'infverif', 'stampinf', 'makecert', 'signtool'
            )) {
            Assert-Command $command
        }
        Invoke-Checked cargo @('make', 'default', '--target', 'x86_64-pc-windows-msvc')
    }
    elseif ($Action -eq 'ValidatePackage') {
        foreach ($command in @('cargo-make', 'inf2cat', 'infverif', 'stampinf')) {
            Assert-Command $command
        }
        Invoke-Checked cargo @(
            'make', 'package-unsigned', '--target', 'x86_64-pc-windows-msvc'
        )
    }
    else {
        Invoke-Checked cargo @('fmt', '--all', '--', '--check')
        Invoke-Checked cargo @(
            'check', '--target', 'x86_64-pc-windows-msvc', '--locked'
        )
        Invoke-Checked cargo @(
            'test', '--target', 'x86_64-pc-windows-msvc', '--locked'
        )
        Invoke-Checked cargo @(
            'clippy', '--all-targets', '--target', 'x86_64-pc-windows-msvc', '--locked',
            '--', '-D', 'warnings'
        )
    }
}
finally {
    Pop-Location
}
