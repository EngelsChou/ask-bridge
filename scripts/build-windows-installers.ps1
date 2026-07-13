# Build portable install.exe and uninstall.exe for offline Windows computers.
param(
    [string] $OutputDirectory = ""
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

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    throw "cargo was not found. Rust is required on the build computer only."
}

$OriginalRustFlags = $env:RUSTFLAGS
$OriginalAppPath = $env:ASK_BRIDGE_BINARY_PATH
$OriginalUpdatePath = $env:ASK_BRIDGE_UPDATE_BINARY_PATH
$OriginalUninstallPath = $env:ASK_BRIDGE_UNINSTALL_BINARY_PATH
$OriginalAppVersion = $env:ASK_BRIDGE_APP_VERSION
$OriginalPayloadFingerprint = $env:ASK_BRIDGE_PAYLOAD_FINGERPRINT

try {
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

    $CargoToml = Get-Content -Raw -Encoding UTF8 (Join-Path $ProjectRoot "Cargo.toml")
    $VersionMatch = [regex]::Match($CargoToml, '(?m)^\s*version\s*=\s*"([^"]+)"')
    if (-not $VersionMatch.Success) {
        throw "Could not read the Ask Bridge version from Cargo.toml."
    }

    $env:ASK_BRIDGE_BINARY_PATH = [System.IO.Path]::GetFullPath($AskBridgePath)
    $env:ASK_BRIDGE_UPDATE_BINARY_PATH = [System.IO.Path]::GetFullPath($UpdatePath)
    $env:ASK_BRIDGE_UNINSTALL_BINARY_PATH = [System.IO.Path]::GetFullPath($UninstallPath)
    $env:ASK_BRIDGE_APP_VERSION = $VersionMatch.Groups[1].Value
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
}
