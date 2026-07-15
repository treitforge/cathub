[CmdletBinding()]
param()

& (Join-Path $PSScriptRoot 'build.ps1') test
exit $LASTEXITCODE
