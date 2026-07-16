# Build portable install.exe and uninstall.exe for offline Windows computers.
param(
    [string] $OutputDirectory = "",
    [string] $SignTool = "",
    [string] $CertificateThumbprint = "",
    [string] $CertificatePath = "",
    [string] $CertificatePassword = "",
    [string] $TimestampUrl = "http://timestamp.digicert.com",
    [switch] $RequireSignature
)

$ErrorActionPreference = "Stop"

if ($env:OS -ne "Windows_NT") {
    throw "The Windows offline installers can only be built on Windows."
}

$ProjectRoot = Split-Path -Parent $PSScriptRoot
$InstallerManifest = Join-Path $ProjectRoot "windows-installer\Cargo.toml"
$InstallerTarget = Join-Path $ProjectRoot "windows-installer\target\release"
$ApplicationTarget = Join-Path $ProjectRoot "target\release"
if ([string]::IsNullOrWhiteSpace($OutputDirectory)) {
    $OutputDirectory = Join-Path $ProjectRoot "dist\windows"
} elseif (-not [System.IO.Path]::IsPathRooted($OutputDirectory)) {
    $OutputDirectory = Join-Path $ProjectRoot $OutputDirectory
}
$OutputDirectory = [System.IO.Path]::GetFullPath($OutputDirectory)

function Invoke-CargoCommand {
    param([string[]] $CargoArguments)

    & cargo @CargoArguments
    if ($LASTEXITCODE -ne 0) {
        throw "cargo $($CargoArguments -join ' ') failed with exit code $LASTEXITCODE"
    }
}

function Resolve-Executable {
    param(
        [string] $ExplicitPath,
        [Parameter(Mandatory)][string] $CommandName,
        [string[]] $FallbackPaths = @()
    )

    if ($ExplicitPath) {
        $resolved = [System.IO.Path]::GetFullPath($ExplicitPath)
        if (-not (Test-Path -LiteralPath $resolved -PathType Leaf)) {
            throw "Executable not found: $resolved"
        }
        return $resolved
    }

    $command = Get-Command $CommandName -ErrorAction SilentlyContinue
    if ($command) {
        return $command.Source
    }

    foreach ($candidate in $FallbackPaths) {
        if ($candidate -and (Test-Path -LiteralPath $candidate -PathType Leaf)) {
            return [System.IO.Path]::GetFullPath($candidate)
        }
    }

    return $null
}

function Get-CertificateSha256 {
    param([Parameter(Mandatory)] [Security.Cryptography.X509Certificates.X509Certificate2] $Certificate)

    $sha256 = [Security.Cryptography.SHA256]::Create()
    try {
        return ([BitConverter]::ToString($sha256.ComputeHash($Certificate.RawData))).Replace('-', '').ToLowerInvariant()
    } finally {
        $sha256.Dispose()
    }
}

function Assert-ExpectedPublisherCertificate {
    param([Parameter(Mandatory)] [Security.Cryptography.X509Certificates.X509Certificate2] $Certificate)

    $publisher = $Certificate.GetNameInfo(
        [Security.Cryptography.X509Certificates.X509NameType]::SimpleName,
        $false
    )
    if ($publisher -cne "Engels Chou") {
        throw "The code-signing certificate publisher must be exactly 'Engels Chou'; got '$publisher'."
    }
    $actualSignerSha256 = Get-CertificateSha256 -Certificate $Certificate
    if ($env:ASK_BRIDGE_WINDOWS_SIGNER_SHA256 -and $actualSignerSha256 -cne $env:ASK_BRIDGE_WINDOWS_SIGNER_SHA256.ToLowerInvariant()) {
        throw "The code-signing certificate does not match ASK_BRIDGE_WINDOWS_SIGNER_SHA256."
    }
    $env:ASK_BRIDGE_WINDOWS_SIGNER_SHA256 = $actualSignerSha256
    return $actualSignerSha256
}

function Resolve-SigningConfiguration {
    if (-not $CertificateThumbprint) {
        $script:CertificateThumbprint = $env:ASK_BRIDGE_SIGNING_CERTIFICATE_THUMBPRINT
    }
    if (-not $CertificatePath) {
        $script:CertificatePath = $env:ASK_BRIDGE_SIGNING_CERTIFICATE_PATH
    }
    if (-not $CertificatePassword) {
        $script:CertificatePassword = $env:ASK_BRIDGE_SIGNING_CERTIFICATE_PASSWORD
    }

    if ($CertificateThumbprint -and $CertificatePath) {
        throw "Specify either CertificateThumbprint or CertificatePath, not both."
    }
    if (-not $CertificateThumbprint -and -not $CertificatePath) {
        if ($RequireSignature) {
            throw "A code-signing certificate is required. Pass -CertificateThumbprint or -CertificatePath, or configure the ASK_BRIDGE_SIGNING_CERTIFICATE_* environment variables."
        }
        return $null
    }

    $windowsKitsRoot = Join-Path ${env:ProgramFiles(x86)} "Windows Kits\10\bin"
    $kitSignTool = Get-ChildItem -Path (Join-Path $windowsKitsRoot "*\x64\signtool.exe") -File -ErrorAction SilentlyContinue |
        Sort-Object FullName -Descending |
        Select-Object -First 1
    $signToolFallbacks = @(
        $(if ($kitSignTool) { $kitSignTool.FullName }),
        (Join-Path $env:ProgramFiles "Windows Kits\10\App Certification Kit\signtool.exe")
    )
    $resolvedSignTool = Resolve-Executable -ExplicitPath $SignTool -CommandName "signtool.exe" -FallbackPaths $signToolFallbacks
    if (-not $resolvedSignTool) {
        throw "A signing certificate was configured, but signtool.exe was not found. Install the Windows SDK or pass -SignTool <path>."
    }

    if ($CertificatePath) {
        $script:CertificatePath = [System.IO.Path]::GetFullPath($CertificatePath)
        if (-not (Test-Path -LiteralPath $CertificatePath -PathType Leaf)) {
            throw "Code-signing certificate file not found: $CertificatePath"
        }
        $certificateBytes = [IO.File]::ReadAllBytes($CertificatePath)
        $certificate = [Security.Cryptography.X509Certificates.X509Certificate2]::new(
            $certificateBytes,
            $CertificatePassword,
            [Security.Cryptography.X509Certificates.X509KeyStorageFlags]::EphemeralKeySet
        )
    } else {
        $normalizedThumbprint = $CertificateThumbprint -replace '\s', ''
        $certificate = Get-ChildItem -LiteralPath "Cert:\CurrentUser\My\$normalizedThumbprint" -ErrorAction Stop
    }
    if (-not $certificate.HasPrivateKey) {
        throw "The configured code-signing certificate has no private key."
    }
    $script:ExpectedSignerSha256 = Assert-ExpectedPublisherCertificate -Certificate $certificate

    return $resolvedSignTool
}

function Invoke-CodeSigning {
    param(
        [Parameter(Mandatory)][string] $ResolvedSignTool,
        [Parameter(Mandatory)][string] $Path,
        [Parameter(Mandatory)][string] $Description
    )

    $arguments = @("sign", "/fd", "SHA256", "/td", "SHA256", "/tr", $TimestampUrl, "/d", $Description, "/v")
    if ($CertificateThumbprint) {
        $arguments += @("/s", "My", "/sha1", ($CertificateThumbprint -replace '\s', ''))
    } else {
        $arguments += @("/f", $CertificatePath)
        if ($CertificatePassword) {
            $arguments += @("/p", $CertificatePassword)
        }
    }
    $arguments += $Path

    & $ResolvedSignTool @arguments
    if ($LASTEXITCODE -ne 0) {
        throw "Authenticode signing failed for $Path with exit code $LASTEXITCODE."
    }
    & $ResolvedSignTool verify /pa /v $Path
    if ($LASTEXITCODE -ne 0) {
        throw "Authenticode verification failed for $Path with exit code $LASTEXITCODE."
    }
    $signature = Get-AuthenticodeSignature -LiteralPath $Path
    if ($signature.Status -ne [System.Management.Automation.SignatureStatus]::Valid) {
        throw "PowerShell Authenticode verification failed for ${Path}: $($signature.Status) $($signature.StatusMessage)"
    }
    $actualSignerSha256 = Assert-ExpectedPublisherCertificate -Certificate $signature.SignerCertificate
    if ($actualSignerSha256 -cne $script:ExpectedSignerSha256) {
        throw "The signed file $Path uses an unexpected signer certificate."
    }
}

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    throw "cargo was not found. Rust is required on the build computer only."
}

$ResolvedSignTool = Resolve-SigningConfiguration

$CargoToml = Get-Content -Raw -Encoding UTF8 (Join-Path $ProjectRoot "Cargo.toml")
$VersionMatch = [regex]::Match($CargoToml, '(?m)^\s*version\s*=\s*"([^"]+)"')
if (-not $VersionMatch.Success) {
    throw "Could not read the Ask Bridge version from Cargo.toml."
}
$BuildAppVersion = $VersionMatch.Groups[1].Value

$OriginalRustFlags = $env:RUSTFLAGS
$OriginalAppPath = $env:ASK_BRIDGE_BINARY_PATH
$OriginalUpdatePath = $env:ASK_BRIDGE_UPDATE_BINARY_PATH
$OriginalUninstallPath = $env:ASK_BRIDGE_UNINSTALL_BINARY_PATH
$OriginalAppVersion = $env:ASK_BRIDGE_APP_VERSION
$OriginalPayloadFingerprint = $env:ASK_BRIDGE_PAYLOAD_FINGERPRINT
$OriginalSignerSha256 = $env:ASK_BRIDGE_WINDOWS_SIGNER_SHA256

try {
    $env:ASK_BRIDGE_APP_VERSION = $BuildAppVersion
    if ($env:RUSTFLAGS -notmatch 'target-feature=\+crt-static') {
        $env:RUSTFLAGS = (($env:RUSTFLAGS, "-C target-feature=+crt-static") -join " ").Trim()
    }

    Push-Location $ProjectRoot
    try {
        Write-Host "Building statically linked Ask Bridge release binaries..." -ForegroundColor Cyan
        Invoke-CargoCommand -CargoArguments @("build", "--release", "--locked")
    } finally {
        Pop-Location
    }

    $AskBridgePath = Join-Path $ApplicationTarget "ask-bridge.exe"
    $UpdatePath = Join-Path $ApplicationTarget "ask-bridge-update.exe"
    foreach ($RequiredPath in @($AskBridgePath, $UpdatePath)) {
        if (-not (Test-Path -LiteralPath $RequiredPath -PathType Leaf)) {
            throw "Missing build output: $RequiredPath"
        }
    }

    Write-Host "Building uninstaller..." -ForegroundColor Cyan
    Invoke-CargoCommand -CargoArguments @(
        "build",
        "--manifest-path", $InstallerManifest,
        "--release",
        "--locked",
        "--bin", "uninstall"
    )
    $UninstallPath = Join-Path $InstallerTarget "uninstall.exe"
    if (-not (Test-Path -LiteralPath $UninstallPath -PathType Leaf)) {
        throw "Missing uninstaller: $UninstallPath"
    }

    if ($ResolvedSignTool) {
        Invoke-CodeSigning -ResolvedSignTool $ResolvedSignTool -Path $AskBridgePath -Description "Ask Bridge"
        Invoke-CodeSigning -ResolvedSignTool $ResolvedSignTool -Path $UpdatePath -Description "Ask Bridge Update"
        Invoke-CodeSigning -ResolvedSignTool $ResolvedSignTool -Path $UninstallPath -Description "Ask Bridge Uninstaller"
    }

    $env:ASK_BRIDGE_BINARY_PATH = [System.IO.Path]::GetFullPath($AskBridgePath)
    $env:ASK_BRIDGE_UPDATE_BINARY_PATH = [System.IO.Path]::GetFullPath($UpdatePath)
    $env:ASK_BRIDGE_UNINSTALL_BINARY_PATH = [System.IO.Path]::GetFullPath($UninstallPath)
    $env:ASK_BRIDGE_APP_VERSION = $BuildAppVersion
    $PayloadHashes = @(
        (Get-FileHash -LiteralPath $AskBridgePath -Algorithm SHA256).Hash,
        (Get-FileHash -LiteralPath $UpdatePath -Algorithm SHA256).Hash,
        (Get-FileHash -LiteralPath $UninstallPath -Algorithm SHA256).Hash
    )
    $env:ASK_BRIDGE_PAYLOAD_FINGERPRINT = $PayloadHashes -join ":"

    Write-Host "Embedding all binaries in install.exe..." -ForegroundColor Cyan
    Invoke-CargoCommand -CargoArguments @(
        "build",
        "--manifest-path", $InstallerManifest,
        "--release",
        "--locked",
        "--bin", "install"
    )
    $InstallPath = Join-Path $InstallerTarget "install.exe"
    if (-not (Test-Path -LiteralPath $InstallPath -PathType Leaf)) {
        throw "Missing installer: $InstallPath"
    }
    if ($ResolvedSignTool) {
        Invoke-CodeSigning -ResolvedSignTool $ResolvedSignTool -Path $InstallPath -Description "Ask Bridge Installer"
    }

    New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null
    $Outputs = @(
        @{ Source = $InstallPath; Name = "install.exe" },
        @{ Source = $UninstallPath; Name = "uninstall.exe" }
    )
    foreach ($Output in $Outputs) {
        $Destination = Join-Path $OutputDirectory $Output.Name
        Copy-Item -LiteralPath $Output.Source -Destination $Destination -Force
        $Hash = (Get-FileHash -LiteralPath $Destination -Algorithm SHA256).Hash.ToLowerInvariant()
        "$Hash  $($Output.Name)" | Out-File "$Destination.sha256" -Encoding ascii
        Write-Host "Created: $Destination" -ForegroundColor Green
        Write-Host "SHA-256: $Hash" -ForegroundColor DarkGray
    }
} finally {
    $env:RUSTFLAGS = $OriginalRustFlags
    $env:ASK_BRIDGE_BINARY_PATH = $OriginalAppPath
    $env:ASK_BRIDGE_UPDATE_BINARY_PATH = $OriginalUpdatePath
    $env:ASK_BRIDGE_UNINSTALL_BINARY_PATH = $OriginalUninstallPath
    $env:ASK_BRIDGE_APP_VERSION = $OriginalAppVersion
    $env:ASK_BRIDGE_PAYLOAD_FINGERPRINT = $OriginalPayloadFingerprint
    $env:ASK_BRIDGE_WINDOWS_SIGNER_SHA256 = $OriginalSignerSha256
}
