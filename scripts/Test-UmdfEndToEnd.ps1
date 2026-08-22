[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [switch]$IUnderstandThisInstallsATestDriver,

    [string]$DriverPackage = (Join-Path $PSScriptRoot 'driver'),
    [string]$CatHubExe = (Join-Path $PSScriptRoot 'cathub.exe'),
    [string]$DevConExe = (Join-Path $PSScriptRoot 'devcon.exe'),
    [string]$ResultsPath = (Join-Path $PSScriptRoot 'cathub-umdf-e2e.json')
)

$ErrorActionPreference = 'Stop'

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

function Find-CatHubPort {
    $deadline = [DateTime]::UtcNow.AddSeconds(20)
    do {
        $device = Get-PnpDevice -Class Ports -PresentOnly -ErrorAction SilentlyContinue |
            Where-Object InstanceId -Like 'ROOT\CATHUB_VIRTUAL_SERIAL*' |
            Select-Object -First 1
        if ($device) {
            $match = [regex]::Match($device.FriendlyName, '\((COM\d+)\)')
            if ($match.Success) {
                return $match.Groups[1].Value
            }

            $enumPath = "HKLM:\SYSTEM\CurrentControlSet\Enum\$($device.InstanceId)"
            foreach ($path in @($enumPath, (Join-Path $enumPath 'Device Parameters'))) {
                $portName = (Get-ItemProperty -LiteralPath $path -Name PortName `
                        -ErrorAction SilentlyContinue).PortName
                if ($portName -match '^COM\d+$') {
                    return $portName
                }
            }
        }
        Start-Sleep -Milliseconds 250
    } while ([DateTime]::UtcNow -lt $deadline)

    throw 'The CatHub virtual COM port did not appear within 20 seconds.'
}

function Invoke-CatQuery {
    param(
        [Parameter(Mandatory)][System.IO.Ports.SerialPort]$Port,
        [Parameter(Mandatory)][string]$Command
    )

    $Port.Write($Command)
    return $Port.ReadTo(';') + ';'
}

if (-not $IUnderstandThisInstallsATestDriver) {
    throw 'Pass -IUnderstandThisInstallsATestDriver on an isolated test VM.'
}

$computer = Get-CimInstance Win32_ComputerSystem
if ($computer.Manufacturer -ne 'Microsoft Corporation' -or $computer.Model -ne 'Virtual Machine') {
    throw 'This test installs a private test certificate and driver and is restricted to a Hyper-V VM.'
}

$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = [Security.Principal.WindowsPrincipal]::new($identity)
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'Run this test from an elevated PowerShell session inside the isolated VM.'
}

$infPath = Join-Path $DriverPackage 'cathub_virtual_serial_umdf.inf'
$certificatePath = Join-Path $DriverPackage 'cathub_umdf_test.cer'
foreach ($path in @($infPath, $certificatePath, $CatHubExe, $DevConExe)) {
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
        throw "Required test input is missing: $path"
    }
}

$workRoot = Join-Path $env:TEMP 'cathub-umdf-e2e'
$configPath = Join-Path $workRoot 'cathub.toml'
$stdoutPath = Join-Path $workRoot 'cathub.stdout.log'
$stderrPath = Join-Path $workRoot 'cathub.stderr.log'
$null = New-Item -ItemType Directory -Path $workRoot -Force

@'
[radio]
backend = "loopback"
model = "TS-590SG"

[[serial_endpoint]]
name = "umdf-e2e"
virtual_endpoint = "cathub-default"
dialect = "ts590"
perms = ["read", "frequency_write", "write", "ptt", "config_write"]
'@ | Set-Content -LiteralPath $configPath -Encoding utf8

$certificate = [System.Security.Cryptography.X509Certificates.X509Certificate2]::new(
    $certificatePath
)
Invoke-Checked certutil @('-f', '-addstore', 'Root', $certificatePath)
Invoke-Checked certutil @('-f', '-addstore', 'TrustedPublisher', $certificatePath)
Invoke-Checked $DevConExe @('install', $infPath, 'root\CATHUB_VIRTUAL_SERIAL')

$portName = Find-CatHubPort
$process = $null
$serial = $null
$results = [ordered]@{
    timestamp_utc = [DateTime]::UtcNow.ToString('o')
    machine = $env:COMPUTERNAME
    port = $portName
    driver_certificate = $certificate.Thumbprint
    cases = @()
}

try {
    $process = Start-Process -FilePath $CatHubExe `
        -ArgumentList @('--config', $configPath) `
        -RedirectStandardOutput $stdoutPath `
        -RedirectStandardError $stderrPath `
        -PassThru
    Start-Sleep -Seconds 2
    if ($process.HasExited) {
        throw "CatHub exited during startup with code $($process.ExitCode)."
    }

    $serial = [System.IO.Ports.SerialPort]::new(
        $portName,
        9600,
        [System.IO.Ports.Parity]::None,
        8,
        [System.IO.Ports.StopBits]::One
    )
    $serial.ReadTimeout = 5000
    $serial.WriteTimeout = 5000
    $serial.DtrEnable = $true
    $serial.RtsEnable = $true
    $serial.Open()
    $serial.DiscardInBuffer()
    $serial.DiscardOutBuffer()

    $serial.ReadTimeout = 100
    $timer = [System.Diagnostics.Stopwatch]::StartNew()
    try {
        $null = $serial.ReadByte()
        throw 'An empty serial read unexpectedly returned data.'
    }
    catch [System.TimeoutException] {
        $timer.Stop()
    }
    if ($timer.ElapsedMilliseconds -lt 50 -or $timer.ElapsedMilliseconds -gt 1000) {
        throw "Empty read timed out after $($timer.ElapsedMilliseconds) ms."
    }
    $results.cases += [ordered]@{
        name = 'read_timeout'
        passed = $true
        elapsed_ms = $timer.ElapsedMilliseconds
    }
    $serial.ReadTimeout = 5000

    $id = Invoke-CatQuery -Port $serial -Command 'ID;'
    if ($id -ne 'ID021;') {
        throw "Unexpected ID response '$id'."
    }
    $results.cases += [ordered]@{ name = 'initial_id_query'; passed = $true; response = $id }

    $serial.Write('FA00014074000;')
    $frequency = Invoke-CatQuery -Port $serial -Command 'FA;'
    if ($frequency -ne 'FA00014074000;') {
        throw "Unexpected frequency response '$frequency'."
    }
    $results.cases += [ordered]@{
        name = 'bidirectional_frequency_round_trip'
        passed = $true
        response = $frequency
    }

    $serial.Close()
    $serial.Dispose()
    $serial = $null
    Start-Sleep -Seconds 1

    $serial = [System.IO.Ports.SerialPort]::new($portName, 4800)
    $serial.ReadTimeout = 5000
    $serial.WriteTimeout = 5000
    $serial.Open()
    $reopenedId = Invoke-CatQuery -Port $serial -Command 'ID;'
    if ($reopenedId -ne 'ID021;') {
        throw "Unexpected ID response after reopen '$reopenedId'."
    }
    $results.cases += [ordered]@{
        name = 'close_reopen_reconnect'
        passed = $true
        response = $reopenedId
    }
    $results.passed = $true
}
catch {
    $results.passed = $false
    $results.error = $_.Exception.Message
    throw
}
finally {
    if ($serial) {
        if ($serial.IsOpen) {
            $serial.Close()
        }
        $serial.Dispose()
    }
    if ($process -and -not $process.HasExited) {
        Stop-Process -Id $process.Id -Force
        $process.WaitForExit()
    }
    $results.cathub_stdout = if (Test-Path -LiteralPath $stdoutPath) {
        Get-Content -LiteralPath $stdoutPath -Raw
    } else { '' }
    $results.cathub_stderr = if (Test-Path -LiteralPath $stderrPath) {
        Get-Content -LiteralPath $stderrPath -Raw
    } else { '' }
    $results | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $ResultsPath -Encoding utf8
}

Write-Host "CatHub UMDF end-to-end test passed on $portName."
Write-Host "Results: $ResultsPath"
