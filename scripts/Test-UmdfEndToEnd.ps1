[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [switch]$IUnderstandThisInstallsATestDriver,

    [string]$DriverPackage = (Join-Path $PSScriptRoot 'driver'),
    [string]$CatHubExe = (Join-Path $PSScriptRoot 'cathub.exe'),
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

function Find-CatHubDevice {
    $deadline = [DateTime]::UtcNow.AddSeconds(20)
    do {
        $device = Get-PnpDevice -Class Ports -PresentOnly -ErrorAction SilentlyContinue |
            Where-Object InstanceId -Like 'ROOT\CATHUB_VIRTUAL_SERIAL*' |
            Select-Object -First 1
        if ($device) {
            $match = [regex]::Match($device.FriendlyName, '\((COM\d+)\)')
            if ($match.Success) {
                return [pscustomobject]@{
                    InstanceId = $device.InstanceId
                    Port = $match.Groups[1].Value
                }
            }

            $enumPath = "HKLM:\SYSTEM\CurrentControlSet\Enum\$($device.InstanceId)"
            foreach ($path in @($enumPath, (Join-Path $enumPath 'Device Parameters'))) {
                $portName = (Get-ItemProperty -LiteralPath $path -Name PortName `
                        -ErrorAction SilentlyContinue).PortName
                if ($portName -match '^COM\d+$') {
                    return [pscustomobject]@{
                        InstanceId = $device.InstanceId
                        Port = $portName
                    }
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
foreach ($path in @($infPath, $certificatePath, $CatHubExe)) {
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
application_transport = "COM91"
dialect = "ts590"
perms = ["read", "frequency_write", "write", "ptt", "config_write"]
'@ | Set-Content -LiteralPath $configPath -Encoding utf8

$certificate = [System.Security.Cryptography.X509Certificates.X509Certificate2]::new(
    $certificatePath
)
Invoke-Checked certutil @('-f', '-addstore', 'Root', $certificatePath)
Invoke-Checked certutil @('-f', '-addstore', 'TrustedPublisher', $certificatePath)
Invoke-Checked $CatHubExe @(
    '--config', $configPath,
    'virtual-serial', 'apply',
    '--inf', $infPath,
    '--format', 'json'
)

$device = Find-CatHubDevice
$portName = $device.Port
$process = $null
$serial = $null
$results = [ordered]@{
    timestamp_utc = [DateTime]::UtcNow.ToString('o')
    machine = $env:COMPUTERNAME
    port = $portName
    device_instance_id = $device.InstanceId
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

    $stressTimer = [System.Diagnostics.Stopwatch]::StartNew()
    foreach ($iteration in 1..100) {
        $stressId = Invoke-CatQuery -Port $serial -Command 'ID;'
        if ($stressId -ne 'ID021;') {
            throw "Unexpected stress response '$stressId' at iteration $iteration."
        }
    }
    $stressTimer.Stop()
    $results.cases += [ordered]@{
        name = 'repeated_bidirectional_io'
        passed = $true
        iterations = 100
        elapsed_ms = $stressTimer.ElapsedMilliseconds
    }

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

    Stop-Process -Id $process.Id -Force
    $process.WaitForExit()
    $process = $null
    $serial.ReadTimeout = 1000
    $failureTimer = [System.Diagnostics.Stopwatch]::StartNew()
    $boundedFailure = $null
    try {
        $serial.Write('ID;')
        $null = $serial.ReadTo(';')
    }
    catch {
        $boundedFailure = $_.Exception.Message
    }
    $failureTimer.Stop()
    if (-not $boundedFailure) {
        throw 'Application I/O unexpectedly succeeded after the CatHub daemon was terminated.'
    }
    if ($failureTimer.ElapsedMilliseconds -gt 2000) {
        throw "Daemon-loss I/O failure took $($failureTimer.ElapsedMilliseconds) ms."
    }
    $results.cases += [ordered]@{
        name = 'daemon_crash_fails_io_bounded'
        passed = $true
        elapsed_ms = $failureTimer.ElapsedMilliseconds
        error = $boundedFailure
    }
    $serial.Close()
    $serial.Dispose()
    $serial = $null

    $process = Start-Process -FilePath $CatHubExe `
        -ArgumentList @('--config', $configPath) `
        -RedirectStandardOutput $stdoutPath `
        -RedirectStandardError $stderrPath `
        -PassThru
    Start-Sleep -Seconds 2
    if ($process.HasExited) {
        throw "CatHub exited after restart with code $($process.ExitCode)."
    }

    $serial = [System.IO.Ports.SerialPort]::new($portName, 9600)
    $serial.ReadTimeout = 5000
    $serial.WriteTimeout = 5000
    $serial.Open()
    $restartId = Invoke-CatQuery -Port $serial -Command 'ID;'
    if ($restartId -ne 'ID021;') {
        throw "Unexpected ID response after daemon restart '$restartId'."
    }
    $results.cases += [ordered]@{
        name = 'daemon_restart_reconnect'
        passed = $true
        response = $restartId
    }

    Invoke-Checked pnputil @('/restart-device', $device.InstanceId)
    $serial.ReadTimeout = 1000
    $deviceFailureTimer = [System.Diagnostics.Stopwatch]::StartNew()
    $deviceFailure = $null
    try {
        $serial.Write('ID;')
        $null = $serial.ReadTo(';')
    }
    catch {
        $deviceFailure = $_.Exception.Message
    }
    $deviceFailureTimer.Stop()
    if (-not $deviceFailure) {
        throw 'The pre-restart COM handle unexpectedly remained usable after device restart.'
    }
    if ($deviceFailureTimer.ElapsedMilliseconds -gt 3000) {
        throw "Device-restart I/O failure took $($deviceFailureTimer.ElapsedMilliseconds) ms."
    }
    $serial.Close()
    $serial.Dispose()
    $serial = $null

    $restartedDevice = Find-CatHubDevice
    if ($restartedDevice.InstanceId -ne $device.InstanceId) {
        throw "Device restart changed instance ID to '$($restartedDevice.InstanceId)'."
    }

    $reconnectDeadline = [DateTime]::UtcNow.AddSeconds(20)
    $deviceRestartId = $null
    $lastReconnectError = $null
    do {
        try {
            $serial = [System.IO.Ports.SerialPort]::new($portName, 9600)
            $serial.ReadTimeout = 1000
            $serial.WriteTimeout = 1000
            $serial.Open()
            $deviceRestartId = Invoke-CatQuery -Port $serial -Command 'ID;'
            if ($deviceRestartId -eq 'ID021;') {
                break
            }
            $lastReconnectError = "unexpected response '$deviceRestartId'"
        }
        catch {
            $lastReconnectError = $_.Exception.Message
        }
        if ($serial) {
            if ($serial.IsOpen) {
                $serial.Close()
            }
            $serial.Dispose()
            $serial = $null
        }
        Start-Sleep -Milliseconds 500
    } while ([DateTime]::UtcNow -lt $reconnectDeadline)

    if ($deviceRestartId -ne 'ID021;') {
        throw "CatHub did not reconnect after device restart: $lastReconnectError"
    }
    $results.cases += [ordered]@{
        name = 'umdf_device_restart_reconnect'
        passed = $true
        failure_elapsed_ms = $deviceFailureTimer.ElapsedMilliseconds
        failure_error = $deviceFailure
        response = $deviceRestartId
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
