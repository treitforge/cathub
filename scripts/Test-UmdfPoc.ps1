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

function Find-SignTool {
    $command = Get-Command signtool -ErrorAction SilentlyContinue
    if ($command) {
        return $command.Source
    }

    $kitsBin = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10\bin'
    if (Test-Path -LiteralPath $kitsBin) {
        $candidate = Get-ChildItem -LiteralPath $kitsBin -Directory |
            Where-Object { Test-Path -LiteralPath (Join-Path $_.FullName 'x64\signtool.exe') } |
            Sort-Object { [version]$_.Name } -Descending |
            Select-Object -First 1
        if ($candidate) {
            return Join-Path $candidate.FullName 'x64\signtool.exe'
        }
    }

    throw 'Required UMDF build command signtool is not available. Install the Windows SDK signing tools.'
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

function New-EphemeralDriverCertificate {
    param(
        [Parameter(Mandatory)][string]$PfxPath,
        [Parameter(Mandatory)][string]$CerPath,
        [Parameter(Mandatory)][string]$Password
    )

    $rsa = [System.Security.Cryptography.RSA]::Create(3072)
    try {
        $request = [System.Security.Cryptography.X509Certificates.CertificateRequest]::new(
            'CN=CatHub UMDF Test Certificate',
            $rsa,
            [System.Security.Cryptography.HashAlgorithmName]::SHA256,
            [System.Security.Cryptography.RSASignaturePadding]::Pkcs1
        )
        $request.CertificateExtensions.Add(
            [System.Security.Cryptography.X509Certificates.X509BasicConstraintsExtension]::new(
                $false, $false, 0, $true
            )
        )
        $request.CertificateExtensions.Add(
            [System.Security.Cryptography.X509Certificates.X509KeyUsageExtension]::new(
                [System.Security.Cryptography.X509Certificates.X509KeyUsageFlags]::DigitalSignature,
                $true
            )
        )
        $usages = [System.Security.Cryptography.OidCollection]::new()
        $null = $usages.Add([System.Security.Cryptography.Oid]::new(
                '1.3.6.1.5.5.7.3.3', 'Code Signing'
            ))
        $request.CertificateExtensions.Add(
            [System.Security.Cryptography.X509Certificates.X509EnhancedKeyUsageExtension]::new(
                $usages, $true
            )
        )

        $certificate = $request.CreateSelfSigned(
            [DateTimeOffset]::UtcNow.AddMinutes(-5),
            [DateTimeOffset]::UtcNow.AddDays(30)
        )
        try {
            [System.IO.File]::WriteAllBytes(
                $PfxPath,
                $certificate.Export(
                    [System.Security.Cryptography.X509Certificates.X509ContentType]::Pfx,
                    $Password
                )
            )
            [System.IO.File]::WriteAllBytes(
                $CerPath,
                $certificate.Export(
                    [System.Security.Cryptography.X509Certificates.X509ContentType]::Cert
                )
            )
        }
        finally {
            $certificate.Dispose()
        }
    }
    finally {
        $rsa.Dispose()
    }
}

function Write-PackageManifest {
    param(
        [Parameter(Mandatory)][string]$PackageRoot,
        [Parameter(Mandatory)][string]$CertificatePath
    )

    $certificate = [System.Security.Cryptography.X509Certificates.X509Certificate2]::new(
        $CertificatePath
    )
    try {
        $files = @(Get-ChildItem -LiteralPath $PackageRoot -File |
            Where-Object Name -ne 'package-manifest.json' |
            Sort-Object Name |
            ForEach-Object {
                [ordered]@{
                    name = $_.Name
                    length = $_.Length
                    sha256 = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash
                }
            })
        $manifest = [ordered]@{
            schema_version = 1
            generated_at_utc = [DateTime]::UtcNow.ToString('o')
            package = 'cathub-virtual-serial-umdf'
            architecture = 'x86_64-pc-windows-msvc'
            git_revision = (& git -C $driverRoot rev-parse HEAD).Trim()
            rustc = (& rustc --version).Trim()
            windows_drivers_rs_revision = '8e88dd899d9fa988df841e08cc01e9f663e5a415'
            wdk_nuget = 'Microsoft.Windows.WDK.x64 10.0.28000.2526'
            umdf = '2.33'
            signing = [ordered]@{
                purpose = 'isolated development test only'
                subject = $certificate.Subject
                thumbprint = $certificate.Thumbprint
                not_after_utc = $certificate.NotAfter.ToUniversalTime().ToString('o')
            }
            files = $files
        }
        $json = $manifest | ConvertTo-Json -Depth 6
        [System.IO.File]::WriteAllText(
            (Join-Path $PackageRoot 'package-manifest.json'),
            $json,
            [System.Text.UTF8Encoding]::new($false)
        )
    }
    finally {
        $certificate.Dispose()
    }
}

Assert-Command cargo
Assert-Command clang
Initialize-WdkEnvironment

Push-Location $driverRoot
try {
    if ($Action -eq 'Package') {
        foreach ($command in @('cargo-make', 'inf2cat', 'infverif', 'stampinf')) {
            Assert-Command $command
        }
        $signTool = Find-SignTool
        Invoke-Checked cargo @(
            'make', 'package-unsigned', '--target', 'x86_64-pc-windows-msvc'
        )

        $packageRoot = Join-Path $driverRoot `
            'target\x86_64-pc-windows-msvc\debug\cathub_virtual_serial_umdf_package'
        $catalogPath = Join-Path $packageRoot 'cathub_virtual_serial_umdf.cat'
        $certificatePath = Join-Path $packageRoot 'cathub_umdf_test.cer'
        $manifestPath = Join-Path $packageRoot 'package-manifest.json'
        if (Test-Path -LiteralPath $manifestPath) {
            Remove-Item -LiteralPath $manifestPath -Force
        }
        $pfxPath = Join-Path ([System.IO.Path]::GetTempPath()) `
            "cathub-umdf-$([guid]::NewGuid().ToString('N')).pfx"
        $password = [guid]::NewGuid().ToString('N')
        try {
            New-EphemeralDriverCertificate `
                -PfxPath $pfxPath `
                -CerPath $certificatePath `
                -Password $password
            Invoke-Checked $signTool @(
                'sign', '/v', '/fd', 'SHA256', '/f', $pfxPath, '/p', $password, $catalogPath
            )
            Write-PackageManifest -PackageRoot $packageRoot -CertificatePath $certificatePath
        }
        finally {
            if (Test-Path -LiteralPath $pfxPath) {
                Remove-Item -LiteralPath $pfxPath -Force
            }
        }
        Write-Host "Signed test package: $packageRoot"
        Write-Host 'The public test certificate is included for the isolated target only.'
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
            'build', '--target', 'x86_64-pc-windows-msvc', '--locked'
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
