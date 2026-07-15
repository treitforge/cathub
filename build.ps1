[CmdletBinding()]
param(
    [ValidateSet('build', 'check', 'test', 'proto', 'pack')]
    [string]$Action = 'build',
    [ValidateSet('Debug', 'Release')]
    [string]$Configuration = 'Release'
)

$ErrorActionPreference = 'Stop'
$root = $PSScriptRoot
$cargoProfile = if ($Configuration -eq 'Release') { '--release' } else { $null }

function Invoke-Cargo {
    param([string[]]$CargoArguments)
    & cargo @CargoArguments
    if ($LASTEXITCODE -ne 0) { throw "cargo failed with exit code $LASTEXITCODE" }
}

function Invoke-DotNet {
    param([string[]]$DotNetArguments)
    & dotnet @DotNetArguments
    if ($LASTEXITCODE -ne 0) { throw "dotnet failed with exit code $LASTEXITCODE" }
}

Push-Location $root
try {
    switch ($Action) {
        'build' {
            $cargoArgs = @('build', '--workspace')
            if ($cargoProfile) { $cargoArgs += $cargoProfile }
            Invoke-Cargo -CargoArguments $cargoArgs
            Invoke-DotNet -DotNetArguments @('build', 'CatHub.slnx', '-c', $Configuration)
        }
        'test' {
            Invoke-Cargo -CargoArguments @('test', '--workspace')
            Invoke-DotNet -DotNetArguments @('test', 'CatHub.slnx', '-c', $Configuration)
        }
        'proto' {
            & buf lint
            if ($LASTEXITCODE -ne 0) { throw "buf lint failed with exit code $LASTEXITCODE" }
        }
        'pack' {
            Invoke-Cargo -CargoArguments @('build', '--workspace', '--release')
            Invoke-DotNet -DotNetArguments @('pack', 'src\dotnet\CatHub.Protocol\CatHub.Protocol.csproj', '-c', 'Release', '-o', 'artifacts\packages')
        }
        'check' {
            Invoke-Cargo -CargoArguments @('fmt', '--all', '--', '--check')
            Invoke-Cargo -CargoArguments @('clippy', '--workspace', '--all-targets', '--', '-D', 'warnings')
            Invoke-Cargo -CargoArguments @('test', '--workspace')
            & buf lint
            if ($LASTEXITCODE -ne 0) { throw "buf lint failed with exit code $LASTEXITCODE" }
            Invoke-DotNet -DotNetArguments @('build', 'CatHub.slnx', '-c', $Configuration)
        }
    }
}
finally {
    Pop-Location
}
