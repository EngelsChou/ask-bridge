# install.ps1 for Windows PowerShell
param(
    [switch]$Local,
    [string]$LocalPath = "",
    [string]$MinimumVersion = $env:ASK_BRIDGE_MIN_VERSION,
    [switch]$AllowDowngrade
)

$ErrorActionPreference = "Stop"
$Version = "0.3.10"
$AllowDowngradeRequested = $AllowDowngrade -or $env:ASK_BRIDGE_ALLOW_DOWNGRADE -eq "1"

function ConvertTo-AskBridgeReleaseVersion {
    param(
        [Parameter(Mandatory)] [string] $Value,
        [Parameter(Mandatory)] [string] $Name
    )

    $normalized = $Value.Trim()
    if ($normalized.StartsWith("v", [System.StringComparison]::OrdinalIgnoreCase)) {
        $normalized = $normalized.Substring(1)
    }
    if ($normalized -notmatch '^(\d+)\.(\d+)\.(\d+)$') {
        throw "$Name must be a stable semantic version in MAJOR.MINOR.PATCH form; got '$Value'."
    }

    return [version]$normalized
}

function Assert-AskBridgeVersionTransition {
    param([Parameter(Mandatory)] [string] $ExistingBinary)

    if (-not (Test-Path -LiteralPath $ExistingBinary -PathType Leaf) -or $AllowDowngradeRequested) {
        return
    }

    $InstalledVersionText = (Get-Item -LiteralPath $ExistingBinary).VersionInfo.FileVersion
    if ([string]::IsNullOrWhiteSpace($InstalledVersionText) -or $InstalledVersionText -notmatch '^\s*(\d+)\.(\d+)\.(\d+)(?:\.\d+)?\s*$') {
        throw "Could not safely read the installed ask-bridge file version resource. Set ASK_BRIDGE_ALLOW_DOWNGRADE=1 only if you explicitly want to replace it."
    }
    $InstalledVersion = "$($Matches[1]).$($Matches[2]).$($Matches[3])"
    $InstalledReleaseVersion = ConvertTo-AskBridgeReleaseVersion -Value $InstalledVersion -Name "Installed version"
    $TargetReleaseVersion = ConvertTo-AskBridgeReleaseVersion -Value $Version -Name "Target version"
    if ($TargetReleaseVersion.CompareTo($InstalledReleaseVersion) -lt 0) {
        throw "Refusing to downgrade ask-bridge from $($InstalledReleaseVersion.ToString()) to $Version. Set ASK_BRIDGE_ALLOW_DOWNGRADE=1 only if you explicitly want this downgrade."
    }
}

function Enter-AskBridgeInstallLock {
    param([Parameter(Mandatory)] [string] $InstallDir)

    $LockPath = "$InstallDir.ask-bridge.install.lock"
    $Deadline = [DateTime]::UtcNow.AddSeconds(60)
    while ([DateTime]::UtcNow -lt $Deadline) {
        try {
            $Stream = [System.IO.File]::Open(
                $LockPath,
                [System.IO.FileMode]::OpenOrCreate,
                [System.IO.FileAccess]::ReadWrite,
                [System.IO.FileShare]::None
            )
            return [pscustomobject]@{ Stream = $Stream; Path = $LockPath }
        } catch [System.IO.IOException] {
            Start-Sleep -Milliseconds 200
        }
    }
    throw "Timed out waiting for another Ask Bridge installer to finish ($LockPath)."
}

function Exit-AskBridgeInstallLock {
    param($Lock)

    if (-not $Lock) {
        return
    }
    $Lock.Stream.Dispose()
    Remove-Item -LiteralPath $Lock.Path -Force -ErrorAction SilentlyContinue
}

if (-not [string]::IsNullOrWhiteSpace($MinimumVersion)) {
    $targetReleaseVersion = ConvertTo-AskBridgeReleaseVersion -Value $Version -Name "Target version"
    $minimumReleaseVersion = ConvertTo-AskBridgeReleaseVersion -Value $MinimumVersion -Name "Minimum version"
    if ($targetReleaseVersion.CompareTo($minimumReleaseVersion) -lt 0) {
        Write-Host "Refusing to install ask-bridge $Version because the updater requires $MinimumVersion or newer." -ForegroundColor Red
        exit 1
    }
}

if ($env:ASK_BRIDGE_VERSION_CHECK_ONLY -eq "1") {
    exit 0
}

Write-Host "Starting Ask Bridge installation for Windows..." -ForegroundColor Cyan

function Get-AskBridgeParentPid {
    try {
        $currentPid = $PID
        $seen = @{}

        for ($depth = 0; $depth -lt 16; $depth++) {
            $current = Get-CimInstance Win32_Process -Filter "ProcessId = $currentPid" -ErrorAction SilentlyContinue
            if (-not $current -or -not $current.ParentProcessId) {
                return $null
            }

            if ($seen.ContainsKey([int]$currentPid)) {
                return $null
            }
            $seen[[int]$currentPid] = $true

            $parentPid = [int]$current.ParentProcessId
            $parent = Get-CimInstance Win32_Process -Filter "ProcessId = $parentPid" -ErrorAction SilentlyContinue
            if (-not $parent) {
                return $null
            }

            $parentCommand = $parent.CommandLine
            if ($parent.Name -in @("ask.exe", "ask-bridge.exe")) {
                return [int]$parent.ProcessId
            }

            if ($parentCommand -and $parentCommand -match '\b(?:\.\\)?ask(?:-bridge)?(?:\.exe)?\b.*\bupdate\b') {
                return [int]$parent.ProcessId
            }

            $currentPid = $parentPid
        }
    } catch {
        return $null
    }

    return $null
}

function Stop-AskBridgeParentForUpdate {
    $targetPids = @()
    $parentPid = Get-AskBridgeParentPid
    if ($parentPid) {
        $targetPids += [int]$parentPid
    }

    if ($targetPids.Count -eq 0) {
        try {
            $sessionId = $null
            $self = Get-CimInstance Win32_Process -Filter "ProcessId = $PID" -ErrorAction SilentlyContinue
            if ($self -and $self.SessionId) {
                $sessionId = $self.SessionId
            }

            $allProcesses = Get-CimInstance Win32_Process -Filter "Name='ask.exe' OR Name='ask-bridge.exe'" -ErrorAction SilentlyContinue
            foreach ($process in $allProcesses) {
                if ($sessionId -ne $null -and $process.SessionId -ne $sessionId) {
                    continue
                }
                if ($process.ProcessId -ne $PID) {
                    $targetPids += [int]$process.ProcessId
                }
            }
        } catch {
            Write-Host "Warning: unable to discover running ask processes by fallback scan ($($_.Exception.Message))." -ForegroundColor Yellow
        }
    }

    $targetPids = $targetPids | Sort-Object -Unique
    if ($targetPids.Count -eq 0) {
        return
    }

    foreach ($pid in $targetPids) {
        $targetProcess = Get-CimInstance Win32_Process -Filter "ProcessId = $pid" -ErrorAction SilentlyContinue
        if (-not $targetProcess) {
            continue
        }

        Write-Host "Stopping running ask-bridge process (PID $pid) to replace binaries safely." -ForegroundColor Cyan
        try {
            Stop-Process -Id $pid -Force -ErrorAction Stop
        } catch {
            Write-Host "Warning: failed to stop PID $pid automatically ($($_.Exception.Message))." -ForegroundColor Yellow
        }
    }
}

function Copy-ItemWithRetry {
    param(
        [Parameter(Mandatory)] [string] $Source,
        [Parameter(Mandatory)] [string] $Destination
    )

    $DestinationDirectory = Split-Path -Parent $Destination
    $DestinationName = Split-Path -Leaf $Destination
    for ($attempt = 1; $attempt -le 10; $attempt++) {
        $StagedPath = Join-Path $DestinationDirectory (".$DestinationName.new-" + [guid]::NewGuid().ToString("N"))
        $BackupPath = Join-Path $DestinationDirectory (".$DestinationName.backup-" + [guid]::NewGuid().ToString("N"))
        try {
            Copy-Item -LiteralPath $Source -Destination $StagedPath
            if (Test-Path -LiteralPath $Destination -PathType Leaf) {
                [System.IO.File]::Replace($StagedPath, $Destination, $BackupPath)
            } else {
                [System.IO.File]::Move($StagedPath, $Destination)
            }
            return
        } catch {
            if ($attempt -eq 1) {
                Stop-AskBridgeParentForUpdate
            }

            if ($attempt -eq 10) {
                throw
            }

            Write-Host "Retrying copy for $Destination in 500ms (attempt $attempt/10)..." -ForegroundColor Yellow
            Start-Sleep -Milliseconds 500
        } finally {
            Remove-Item -LiteralPath $StagedPath -Force -ErrorAction SilentlyContinue
            Remove-Item -LiteralPath $BackupPath -Force -ErrorAction SilentlyContinue
        }
    }
}

# 1. Check Node.js and npx
$nodeCheck = Get-Command node -ErrorAction SilentlyContinue
$npxCheck = Get-Command npx -ErrorAction SilentlyContinue

if (-not $nodeCheck) {
    Write-Error "Node.js is not installed. Please install Node.js (https://nodejs.org/) and retry."
    exit 1
}

if (-not $npxCheck) {
    Write-Error "npx is not installed. Please ensure NPM/npx is available in your PATH."
    exit 1
}

$nodeVersionOutput = & node --version 2>&1
$nodeVersionExitCode = $LASTEXITCODE
$nodeVersionText = ($nodeVersionOutput | Out-String).Trim()

if ($nodeVersionExitCode -ne 0 -or $nodeVersionText -notmatch '^v?(\d+)\.(\d+)\.(\d+)(?:[-+].*)?$') {
    Write-Error "Could not determine a supported Node.js version. Install a current Node.js LTS release, reopen PowerShell, and retry."
    exit 1
}

$nodeMajor = [int]$Matches[1]
$nodeMinor = [int]$Matches[2]
$nodePatch = [int]$Matches[3]
$nodeVersionSupported = `
    ($nodeMajor -eq 20 -and ($nodeMinor -gt 19 -or ($nodeMinor -eq 19 -and $nodePatch -ge 0))) -or `
    ($nodeMajor -eq 22 -and ($nodeMinor -gt 12 -or ($nodeMinor -eq 12 -and $nodePatch -ge 0))) -or `
    ($nodeMajor -ge 23)

if (-not $nodeVersionSupported) {
    Write-Error "Node.js $nodeVersionText is not supported by chrome-devtools-mcp@1.5.0. Supported versions are ^20.19.0, ^22.12.0, or >=23.0.0. Install a current Node.js LTS release, reopen PowerShell, and retry."
    exit 1
}

# 2. Check Google Chrome
$chromePaths = @(
    "$env:ProgramFiles\Google\Chrome\Application\chrome.exe",
    "${env:ProgramFiles(x86)}\Google\Chrome\Application\chrome.exe",
    "$env:LocalAppData\Google\Chrome\Application\chrome.exe"
)

$chromeFound = $false
foreach ($path in $chromePaths) {
    if (Test-Path $path) {
        $chromeFound = $true
        break
    }
}

if (-not $chromeFound) {
    Write-Host "Warning: Google Chrome was not found in default installation paths." -ForegroundColor Yellow
    Write-Host "Please ensure Google Chrome is installed, as it is required by Chrome DevTools MCP." -ForegroundColor Yellow
}

# 3. Install from local build (for development)
if ($Local) {
    $LocalRoot = if ($MyInvocation.MyCommand.Path) {
        Split-Path -Parent $MyInvocation.MyCommand.Path
    } else {
        Get-Location
    }

    if ([string]::IsNullOrWhiteSpace($LocalPath)) {
        $LocalPath = Join-Path $LocalRoot "target\release\ask-bridge.exe"
        $LocalUpdatePath = Join-Path $LocalRoot "target\release\ask-bridge-update.exe"
    } else {
        $LocalPath = [System.IO.Path]::GetFullPath($LocalPath)
        $LocalUpdatePath = Join-Path (Split-Path $LocalPath) "ask-bridge-update.exe"
    }

    $LocalPathDir = Split-Path $LocalPath
    if (-not (Test-Path $LocalPathDir)) {
        try {
            $null = New-Item -ItemType Directory -Force -Path $LocalPathDir
        } catch {
            Write-Error "Failed to prepare local build directory '$LocalPathDir'."
            exit 1
        }
    }

    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
        Write-Error "Rust toolchain not found. Please install Rust and retry."
        exit 1
    }

    Write-Host "Building ask-bridge in release mode..." -ForegroundColor Cyan
    try {
        Push-Location $LocalRoot
        & cargo build --release
        if ($LASTEXITCODE -ne 0) {
            Write-Error "cargo build --release failed. Exit code: $LASTEXITCODE"
            exit 1
        }
    } finally {
        Pop-Location
    }

    if (-not (Test-Path $LocalPath)) {
        Write-Error "Local binary not found at '$LocalPath' even after cargo build. Check repository permissions and build output path."
        exit 1
    }
    if (-not (Test-Path $LocalUpdatePath)) {
        Write-Error "Local updater binary not found at '$LocalUpdatePath'. Check repository permissions and build output path."
        exit 1
    }

    $InstallDir = Join-Path $HOME ".local\bin"
    if (-not (Test-Path $InstallDir)) {
        New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
    }

    $DestPath = Join-Path $InstallDir "ask-bridge.exe"
    $AliasPath = Join-Path $InstallDir "ask.exe"
    $UpdatePath = Join-Path $InstallDir "ask-bridge-update.exe"
    $InstallLock = Enter-AskBridgeInstallLock -InstallDir $InstallDir
    try {
        Assert-AskBridgeVersionTransition -ExistingBinary $DestPath
        Write-Host "Installing local ask-bridge.exe to $InstallDir..." -ForegroundColor Cyan
        $ResolvedLocalPath = (Resolve-Path $LocalPath).Path
        $ResolvedLocalUpdatePath = (Resolve-Path $LocalUpdatePath).Path
        Copy-ItemWithRetry -Source $ResolvedLocalUpdatePath -Destination $UpdatePath
        Copy-ItemWithRetry -Source $ResolvedLocalPath -Destination $AliasPath
        # ask-bridge.exe is the commit point and must be replaced last.
        Copy-ItemWithRetry -Source $ResolvedLocalPath -Destination $DestPath
    } finally {
        Exit-AskBridgeInstallLock -Lock $InstallLock
    }

    $UserPath = [Environment]::GetEnvironmentVariable("Path", "User")
    $CleanPathList = $UserPath -split ';'

    if ($CleanPathList -notcontains $InstallDir) {
        Write-Host "Adding $InstallDir to User PATH..." -ForegroundColor Cyan
        $NewPath = $UserPath
        if ($NewPath -and -not $NewPath.EndsWith(';')) {
            $NewPath += ";"
        }
        $NewPath += $InstallDir
        [Environment]::SetEnvironmentVariable("Path", $NewPath, "User")
        
        $env:Path = $env:Path + ";" + $InstallDir
        Write-Host "Successfully added to PATH. You may need to restart your terminal to apply." -ForegroundColor Green
    }

    Write-Host "Successfully installed! You can now use the 'ask-bridge' command. The 'ask' alias is also available." -ForegroundColor Green
    exit 0
}

# 3. Target configuration
$RepoOwner = "EngelsChou"
$RepoName = "ask-bridge"
$ArtifactName = "ask-bridge-x86_64-pc-windows-msvc.zip"
$ReleaseUrl = "https://github.com/$RepoOwner/$RepoName/releases/download/v$Version/$ArtifactName"
$ChecksumUrl = "$ReleaseUrl.sha256"

# 4. Create installation directory
$InstallDir = Join-Path $HOME ".local\bin"
if (-not (Test-Path $InstallDir)) {
    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
}

$ExistingBinary = Join-Path $InstallDir "ask-bridge.exe"
$InstallLock = Enter-AskBridgeInstallLock -InstallDir $InstallDir
try {
    Assert-AskBridgeVersionTransition -ExistingBinary $ExistingBinary
    $TempDir = Join-Path $env:TEMP ("ask-bridge-install-" + [guid]::NewGuid().ToString("N"))
    New-Item -ItemType Directory -Path $TempDir | Out-Null

    try {
    # 5. Download archive and its release checksum
    Write-Host "Downloading $ArtifactName and SHA-256 checksum..." -ForegroundColor Cyan
    $ZipPath = Join-Path $TempDir $ArtifactName
    $ChecksumPath = "$ZipPath.sha256"
    if ([enum]::GetNames([Net.SecurityProtocolType]) -contains "Tls12") {
        [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
    }
    Invoke-WebRequest -UseBasicParsing -Uri $ReleaseUrl -OutFile $ZipPath
    Invoke-WebRequest -UseBasicParsing -Uri $ChecksumUrl -OutFile $ChecksumPath

    $ChecksumLine = (Get-Content -LiteralPath $ChecksumPath -Raw).Trim()
    if ($ChecksumLine -notmatch '(?i)^([0-9a-f]{64})\s+\*?(.+?)\s*$') {
        throw "Invalid checksum file format downloaded from $ChecksumUrl."
    }
    $ExpectedHash = $Matches[1].ToLowerInvariant()
    $ChecksumArtifactName = $Matches[2].Trim()
    if ($ChecksumArtifactName -ne $ArtifactName) {
        throw "Checksum file names '$ChecksumArtifactName', expected '$ArtifactName'."
    }
    $ActualHash = (Get-FileHash -LiteralPath $ZipPath -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($ActualHash -ne $ExpectedHash) {
        throw "SHA-256 checksum mismatch for $ArtifactName. Expected $ExpectedHash, got $ActualHash."
    }
    Write-Host "Verified SHA-256 checksum for $ArtifactName." -ForegroundColor Green

    # 6. Extract the verified archive
    Write-Host "Extracting archive..." -ForegroundColor Cyan
    Expand-Archive -Path $ZipPath -DestinationPath $TempDir -Force

    # Find the executable
    $ExePath = Get-ChildItem -Path $TempDir -Recurse -Filter "ask-bridge.exe" | Select-Object -First 1
    if (-not $ExePath) {
        Write-Error "Could not find ask-bridge.exe in the downloaded archive."
        exit 1
    }
    $UpdateExePath = Get-ChildItem -Path $TempDir -Recurse -Filter "ask-bridge-update.exe" | Select-Object -First 1

    # Copy to destination as ask-bridge.exe and keep ask.exe as an alias.
    Assert-AskBridgeVersionTransition -ExistingBinary $ExistingBinary
    $DestPath = Join-Path $InstallDir "ask-bridge.exe"
    $AliasPath = Join-Path $InstallDir "ask.exe"
    $UpdateDestPath = Join-Path $InstallDir "ask-bridge-update.exe"
    Write-Host "Installing ask-bridge.exe to $InstallDir..." -ForegroundColor Cyan
    if ($UpdateExePath) {
        Write-Host "Installing ask-bridge-update.exe to $InstallDir..." -ForegroundColor Cyan
        Copy-ItemWithRetry -Source $UpdateExePath.FullName -Destination $UpdateDestPath
    } else {
        Write-Host "Warning: ask-bridge-update.exe not found in archive; update helper unavailable." -ForegroundColor Yellow
    }
    Copy-ItemWithRetry -Source $ExePath.FullName -Destination $AliasPath
    # ask-bridge.exe is the commit point and must be replaced last.
    Copy-ItemWithRetry -Source $ExePath.FullName -Destination $DestPath
    }
    finally {
        # Clean up this invocation's private temporary directory.
        if (Test-Path $TempDir) {
            Remove-Item -Recurse -Force $TempDir
        }
    }
} finally {
    Exit-AskBridgeInstallLock -Lock $InstallLock
}

# 7. Check/Add to PATH
$UserPath = [Environment]::GetEnvironmentVariable("Path", "User")
$CleanPathList = $UserPath -split ';'

if ($CleanPathList -notcontains $InstallDir) {
    Write-Host "Adding $InstallDir to User PATH..." -ForegroundColor Cyan
    $NewPath = $UserPath
    if ($NewPath -and -not $NewPath.EndsWith(';')) {
        $NewPath += ";"
    }
    $NewPath += $InstallDir
    [Environment]::SetEnvironmentVariable("Path", $NewPath, "User")
    
    # Update current session path
    $env:Path = $env:Path + ";" + $InstallDir
    Write-Host "Successfully added to PATH. You may need to restart your terminal to apply." -ForegroundColor Green
}

Write-Host "Successfully installed! You can now use the 'ask-bridge' command. The 'ask' alias is also available." -ForegroundColor Green
