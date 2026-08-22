[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [switch]$IUnderstandThisInstallsATestDriver,

    [string]$DriverPackage = (Join-Path $PSScriptRoot 'driver'),
    [string]$CatHubExe = (Join-Path $PSScriptRoot 'cathub.exe'),
    [string]$ConformanceExe = (Join-Path $PSScriptRoot 'serial-conformance.exe'),
    [string]$ResultsPath = (Join-Path $PSScriptRoot 'cathub-umdf-e2e.json'),

    [switch]$KeepInstalled
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

function Invoke-Captured {
    param(
        [Parameter(Mandatory)][string]$Command,
        [Parameter(Mandatory)][string[]]$Arguments
    )

    $output = (& $Command @Arguments 2>&1 | Out-String).Trim()
    return [ordered]@{
        command = "$Command $($Arguments -join ' ')"
        exit_code = $LASTEXITCODE
        output = $output
    }
}

function Get-SignatureEvidence {
    param([Parameter(Mandatory)][string]$Path)

    $signature = Get-AuthenticodeSignature -LiteralPath $Path
    return [ordered]@{
        path = $Path
        status = $signature.Status.ToString()
        status_message = $signature.StatusMessage
        signer_subject = if ($signature.SignerCertificate) {
            $signature.SignerCertificate.Subject
        } else { $null }
        signer_thumbprint = if ($signature.SignerCertificate) {
            $signature.SignerCertificate.Thumbprint
        } else { $null }
        timestamp_subject = if ($signature.TimeStamperCertificate) {
            $signature.TimeStamperCertificate.Subject
        } else { $null }
    }
}

function Get-PackageIntegrityEvidence {
    param(
        [Parameter(Mandatory)][string]$PackageRoot,
        [Parameter(Mandatory)]$Manifest
    )

    $files = foreach ($expected in $Manifest.files) {
        $path = Join-Path $PackageRoot $expected.name
        if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
            [ordered]@{
                name = $expected.name
                exists = $false
                passed = $false
            }
            continue
        }

        $item = Get-Item -LiteralPath $path
        $actualHash = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash
        [ordered]@{
            name = $expected.name
            exists = $true
            expected_length = [long]$expected.length
            actual_length = [long]$item.Length
            expected_sha256 = $expected.sha256
            actual_sha256 = $actualHash
            passed = (
                [long]$item.Length -eq [long]$expected.length -and
                $actualHash -eq $expected.sha256
            )
        }
    }

    return [ordered]@{
        passed = @($files | Where-Object { -not $_.passed }).Count -eq 0
        files = @($files)
    }
}

function Get-SecurityEvidence {
    $secureBoot = $null
    $secureBootError = $null
    try {
        $secureBoot = [bool](Confirm-SecureBootUEFI -ErrorAction Stop)
    }
    catch {
        $secureBootError = $_.Exception.Message
    }

    $deviceGuard = $null
    $deviceGuardError = $null
    try {
        $guard = Get-CimInstance -Namespace 'root\Microsoft\Windows\DeviceGuard' `
            -ClassName Win32_DeviceGuard -ErrorAction Stop
        $deviceGuard = [ordered]@{
            virtualization_based_security_status = $guard.VirtualizationBasedSecurityStatus
            security_services_configured = @($guard.SecurityServicesConfigured)
            security_services_running = @($guard.SecurityServicesRunning)
            available_security_properties = @($guard.AvailableSecurityProperties)
        }
    }
    catch {
        $deviceGuardError = $_.Exception.Message
    }

    $bootPolicy = Invoke-Captured bcdedit.exe @('/enum', '{current}')
    return [ordered]@{
        secure_boot_enabled = $secureBoot
        secure_boot_error = $secureBootError
        device_guard = $deviceGuard
        device_guard_error = $deviceGuardError
        testsigning_enabled = [bool]($bootPolicy.output -match '(?im)^testsigning\s+Yes\s*$')
        no_integrity_checks_enabled = [bool](
            $bootPolicy.output -match '(?im)^nointegritychecks\s+Yes\s*$'
        )
        boot_policy = $bootPolicy
    }
}

function Get-PnpEvidence {
    param([Parameter(Mandatory)][string]$InstanceId)

    $device = Get-PnpDevice -InstanceId $InstanceId -ErrorAction Stop
    $properties = [ordered]@{}
    foreach ($key in @(
        'DEVPKEY_Device_DriverInfPath',
        'DEVPKEY_Device_DriverVersion',
        'DEVPKEY_Device_DriverProvider',
        'DEVPKEY_Device_ProblemCode',
        'DEVPKEY_Device_Service'
    )) {
        $property = Get-PnpDeviceProperty -InstanceId $InstanceId -KeyName $key `
            -ErrorAction SilentlyContinue
        if ($property) {
            $properties[$key] = $property.Data
        }
    }

    return [ordered]@{
        instance_id = $device.InstanceId
        class = $device.Class
        friendly_name = $device.FriendlyName
        status = $device.Status
        problem = $device.Problem
        properties = $properties
    }
}

function Get-EventEvidence {
    param(
        [Parameter(Mandatory)][string]$LogName,
        [Parameter(Mandatory)][DateTime]$StartTime
    )

    try {
        return @(
            Get-WinEvent -FilterHashtable @{ LogName = $LogName; StartTime = $StartTime } `
                -ErrorAction Stop |
                Select-Object TimeCreated, Id, LevelDisplayName, ProviderName, Message
        )
    }
    catch {
        return @([ordered]@{ collection_error = $_.Exception.Message })
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
$catalogPath = Join-Path $DriverPackage 'cathub_virtual_serial_umdf.cat'
$driverDllPath = Join-Path $DriverPackage 'cathub_virtual_serial_umdf.dll'
$manifestPath = Join-Path $DriverPackage 'package-manifest.json'
foreach ($path in @(
    $infPath,
    $certificatePath,
    $catalogPath,
    $driverDllPath,
    $manifestPath,
    $CatHubExe,
    $ConformanceExe
)) {
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
        throw "Required test input is missing: $path"
    }
}

$workRoot = Join-Path $env:TEMP 'cathub-umdf-e2e'
$configPath = Join-Path $workRoot 'cathub.toml'
$stdoutPath = Join-Path $workRoot 'cathub.stdout.log'
$stderrPath = Join-Path $workRoot 'cathub.stderr.log'
$peerStdoutPath = Join-Path $workRoot 'test-peer.stdout.log'
$peerStderrPath = Join-Path $workRoot 'test-peer.stderr.log'
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
$testStart = [DateTime]::UtcNow
$operatingSystem = Get-CimInstance Win32_OperatingSystem
$packageManifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
$device = $null
$portName = $null
$publishedInf = $null
$process = $null
$peerProcess = $null
$serial = $null
$failure = $null
$results = [ordered]@{
    timestamp_utc = $testStart.ToString('o')
    machine = $env:COMPUTERNAME
    os = [ordered]@{
        caption = $operatingSystem.Caption
        version = $operatingSystem.Version
        build_number = $operatingSystem.BuildNumber
    }
    security = Get-SecurityEvidence
    package_manifest = $packageManifest
    package_integrity = Get-PackageIntegrityEvidence `
        -PackageRoot $DriverPackage -Manifest $packageManifest
    cathub_exe_sha256 = (Get-FileHash -LiteralPath $CatHubExe -Algorithm SHA256).Hash
    conformance_exe_sha256 = (Get-FileHash -LiteralPath $ConformanceExe -Algorithm SHA256).Hash
    harness_sha256 = (Get-FileHash -LiteralPath $PSCommandPath -Algorithm SHA256).Hash
    signatures_before_trust = [ordered]@{
        catalog = Get-SignatureEvidence -Path $catalogPath
        driver_dll = Get-SignatureEvidence -Path $driverDllPath
        cathub_exe = Get-SignatureEvidence -Path $CatHubExe
    }
    port = $null
    device_instance_id = $null
    driver_certificate = $certificate.Thumbprint
    keep_installed = [bool]$KeepInstalled
    cases = @()
}

try {
    if (-not $results.package_integrity.passed) {
        throw 'One or more staged driver files do not match package-manifest.json.'
    }
    if ($results.security.secure_boot_enabled -ne $true) {
        throw 'Secure Boot is not enabled in the isolated Windows test target.'
    }
    if ($results.security.testsigning_enabled) {
        throw 'Windows test-signing mode is enabled; the acceptance target must use normal policy.'
    }
    if ($results.security.no_integrity_checks_enabled) {
        throw 'Windows integrity checks are disabled; the acceptance target must use normal policy.'
    }
    if (
        -not $results.security.device_guard -or
        2 -notin @($results.security.device_guard.security_services_running)
    ) {
        throw 'Memory Integrity (HVCI) is not reported as running on the isolated target.'
    }

    $results.certificate_root_install = Invoke-Captured certutil @(
        '-f', '-addstore', 'Root', $certificatePath
    )
    if ($results.certificate_root_install.exit_code -ne 0) {
        throw 'Installing the test certificate in LocalMachine\Root failed.'
    }
    $results.certificate_publisher_install = Invoke-Captured certutil @(
        '-f', '-addstore', 'TrustedPublisher', $certificatePath
    )
    if ($results.certificate_publisher_install.exit_code -ne 0) {
        throw 'Installing the test certificate in LocalMachine\TrustedPublisher failed.'
    }
    $results.signatures_after_trust = [ordered]@{
        catalog = Get-SignatureEvidence -Path $catalogPath
        driver_dll = Get-SignatureEvidence -Path $driverDllPath
        cathub_exe = Get-SignatureEvidence -Path $CatHubExe
    }
    if ($results.signatures_after_trust.catalog.status -ne 'Valid') {
        throw "The installed catalog signature is not valid: $($results.signatures_after_trust.catalog.status_message)"
    }

    $results.apply = Invoke-Captured $CatHubExe @(
        '--config', $configPath,
        'virtual-serial', 'apply',
        '--inf', $infPath,
        '--format', 'json'
    )
    if ($results.apply.exit_code -ne 0) {
        throw "CatHub virtual-serial apply failed: $($results.apply.output)"
    }

    $device = Find-CatHubDevice
    $portName = $device.Port
    $results.port = $portName
    $results.device_instance_id = $device.InstanceId
    $results.pnp_after_install = Get-PnpEvidence -InstanceId $device.InstanceId
    $publishedInf = $results.pnp_after_install.properties['DEVPKEY_Device_DriverInfPath']

    $portProbe = [System.Net.Sockets.TcpListener]::new(
        [System.Net.IPAddress]::Loopback,
        0
    )
    $portProbe.Start()
    $peerPort = ([System.Net.IPEndPoint]$portProbe.LocalEndpoint).Port
    $portProbe.Stop()
    $peerAddress = "127.0.0.1:$peerPort"
    $peerProcess = Start-Process -FilePath $CatHubExe `
        -ArgumentList @(
            'virtual-serial', 'test-peer',
            '--endpoint', 'cathub-default',
            '--kind', 'cat',
            '--listen', $peerAddress
        ) `
        -RedirectStandardOutput $peerStdoutPath `
        -RedirectStandardError $peerStderrPath `
        -PassThru
    Start-Sleep -Seconds 1
    if ($peerProcess.HasExited) {
        throw "The managed serial test peer exited with code $($peerProcess.ExitCode)."
    }

    $results.serial_conformance = [ordered]@{}
    foreach ($profile in @('n1mm-radio', 'n1mm-winkeyer')) {
        $reportPath = Join-Path $workRoot "serial-conformance-$profile.json"
        $run = Invoke-Captured $ConformanceExe @(
            'run',
            '--application-port', $portName,
            '--peer-tcp', $peerAddress,
            '--profile', $profile,
            '--output', $reportPath
        )
        if ($run.exit_code -ne 0) {
            throw "Serial conformance profile '$profile' failed: $($run.output)"
        }
        $results.serial_conformance[$profile] = [ordered]@{
            command = $run
            report = Get-Content -LiteralPath $reportPath -Raw | ConvertFrom-Json
        }
    }
    Stop-Process -Id $peerProcess.Id -Force
    $peerProcess.WaitForExit()
    $peerProcess = $null
    Start-Sleep -Milliseconds 500
    $results.cases += [ordered]@{
        name = 'native_serial_api_conformance'
        passed = $true
        profiles = @('n1mm-radio', 'n1mm-winkeyer')
    }

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
    $results.pnp_after_restart = Get-PnpEvidence -InstanceId $restartedDevice.InstanceId

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
    $failure = $_
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
    if ($peerProcess -and -not $peerProcess.HasExited) {
        Stop-Process -Id $peerProcess.Id -Force
        $peerProcess.WaitForExit()
    }

    if ($results.passed -and -not $KeepInstalled) {
        try {
            $cleanup = [ordered]@{}
            $cleanup.remove_endpoint = Invoke-Captured $CatHubExe @(
                '--config', $configPath,
                'virtual-serial', 'remove',
                '--endpoint', 'cathub-default',
                '--format', 'json'
            )
            if ($cleanup.remove_endpoint.exit_code -ne 0) {
                throw "Removing the CatHub endpoint failed: $($cleanup.remove_endpoint.output)"
            }

            Start-Sleep -Seconds 1
            $remainingDevice = Get-PnpDevice -PresentOnly -ErrorAction SilentlyContinue |
                Where-Object InstanceId -EQ $device.InstanceId
            if ($remainingDevice) {
                throw "CatHub device '$($device.InstanceId)' is still present after removal."
            }

            if ($publishedInf -notmatch '^oem\d+\.inf$') {
                throw "Could not determine the staged OEM INF name (reported '$publishedInf')."
            }
            $cleanup.delete_driver_package = Invoke-Captured pnputil.exe @(
                '/delete-driver', $publishedInf, '/uninstall', '/force'
            )
            if ($cleanup.delete_driver_package.exit_code -ne 0) {
                throw "Deleting driver package '$publishedInf' failed: $($cleanup.delete_driver_package.output)"
            }

            $cleanup.delete_trusted_publisher_certificate = Invoke-Captured certutil.exe @(
                '-delstore', 'TrustedPublisher', $certificate.Thumbprint
            )
            if ($cleanup.delete_trusted_publisher_certificate.exit_code -ne 0) {
                throw 'Removing the test certificate from TrustedPublisher failed.'
            }
            $cleanup.delete_root_certificate = Invoke-Captured certutil.exe @(
                '-delstore', 'Root', $certificate.Thumbprint
            )
            if ($cleanup.delete_root_certificate.exit_code -ne 0) {
                throw 'Removing the test certificate from Root failed.'
            }

            $cleanup.status_after_remove = Invoke-Captured $CatHubExe @(
                'virtual-serial', 'status', '--format', 'json'
            )
            if ($cleanup.status_after_remove.exit_code -ne 0) {
                throw "Post-removal status failed: $($cleanup.status_after_remove.output)"
            }
            $cleanup.passed = $true
            $results.cleanup = $cleanup
            $results.cases += [ordered]@{
                name = 'remove_device_driver_and_test_trust'
                passed = $true
                published_inf = $publishedInf
            }
        }
        catch {
            $results.passed = $false
            $results.error = "Cleanup verification failed: $($_.Exception.Message)"
            $results.cleanup = if ($cleanup) { $cleanup } else { [ordered]@{} }
            $results.cleanup.passed = $false
            $results.cleanup.error = $_.Exception.Message
            $failure = $_
        }
    }
    elseif ($results.passed) {
        $results.cleanup = [ordered]@{
            passed = $null
            skipped = $true
            reason = '-KeepInstalled was specified'
        }
    }

    $results.cathub_stdout = if (Test-Path -LiteralPath $stdoutPath) {
        Get-Content -LiteralPath $stdoutPath -Raw
    } else { '' }
    $results.cathub_stderr = if (Test-Path -LiteralPath $stderrPath) {
        Get-Content -LiteralPath $stderrPath -Raw
    } else { '' }
    $results.test_peer_stdout = if (Test-Path -LiteralPath $peerStdoutPath) {
        Get-Content -LiteralPath $peerStdoutPath -Raw
    } else { '' }
    $results.test_peer_stderr = if (Test-Path -LiteralPath $peerStderrPath) {
        Get-Content -LiteralPath $peerStderrPath -Raw
    } else { '' }
    $results.events = [ordered]@{
        system = Get-EventEvidence -LogName 'System' -StartTime $testStart
        code_integrity = Get-EventEvidence `
            -LogName 'Microsoft-Windows-CodeIntegrity/Operational' -StartTime $testStart
        umdf = Get-EventEvidence `
            -LogName 'Microsoft-Windows-DriverFrameworks-UserMode/Operational' `
            -StartTime $testStart
    }
    $results.completed_at_utc = [DateTime]::UtcNow.ToString('o')
    $results.duration_ms = [long]([DateTime]::UtcNow - $testStart).TotalMilliseconds
    $results | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath $ResultsPath -Encoding utf8
}

if ($failure) {
    throw $failure
}
Write-Host "CatHub UMDF end-to-end test passed on $portName."
Write-Host "Results: $ResultsPath"
