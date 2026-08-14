[CmdletBinding()]
param(
    [ValidateSet('Check', 'Package')]
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

function Assert-WdkHeaders {
    $kitsRoot = (Get-ItemProperty `
        -LiteralPath 'HKLM:\SOFTWARE\Microsoft\Windows Kits\Installed Roots' `
        -ErrorAction SilentlyContinue).KitsRoot10
    if (-not $kitsRoot) {
        throw 'Windows Kits root is not registered. Install a supported Windows Driver Kit.'
    }

    $crtDirectories = @(Get-ChildItem -LiteralPath (Join-Path $kitsRoot 'Include') -Directory |
        Where-Object { Test-Path -LiteralPath (Join-Path $_.FullName 'km\crt') })
    if ($crtDirectories.Count -eq 0) {
        throw "The Windows SDK is present at '$kitsRoot', but WDK km/crt headers are missing."
    }
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
Assert-WdkHeaders

Push-Location $driverRoot
try {
    if ($Action -eq 'Package') {
        foreach ($command in @('cargo-make', 'inf2cat', 'infverif', 'stampinf', 'signtool')) {
            Assert-Command $command
        }
        Invoke-Checked cargo @('make', 'default', '--target', 'x86_64-pc-windows-msvc')
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
