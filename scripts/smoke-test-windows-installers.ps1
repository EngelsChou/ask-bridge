[CmdletBinding()]
param(
    [string] $OutputDirectory = "dist\windows"
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest
$SelfDeletePollMilliseconds = 250
$SelfDeletePollAttempts = 120

if ($env:OS -ne "Windows_NT") {
    throw "The Windows installer smoke test can only run on Windows."
}

$ProjectRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))
if (-not [IO.Path]::IsPathRooted($OutputDirectory)) {
    $OutputDirectory = Join-Path $ProjectRoot $OutputDirectory
}
$OutputDirectory = [IO.Path]::GetFullPath($OutputDirectory)
$Installer = Join-Path $OutputDirectory "install.exe"
if (-not (Test-Path -LiteralPath $Installer -PathType Leaf)) {
    throw "Installer not found: $Installer"
}

$TempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\')
$Target = [IO.Path]::GetFullPath((Join-Path $TempRoot ("ask-bridge-installer-smoke-" + [Guid]::NewGuid().ToString("N"))))
$SafePrefix = $TempRoot + '\'
if (-not $Target.StartsWith($SafePrefix, [StringComparison]::OrdinalIgnoreCase)) {
    throw "Refusing to use an unsafe smoke-test directory: $Target"
}

try {
    & $Installer --install-dir $Target --no-path --quiet
    if ($LASTEXITCODE -ne 0) {
        throw "install.exe exited with code $LASTEXITCODE"
    }

    $InstalledBinary = Join-Path $Target "ask-bridge.exe"
    $InstalledUninstaller = Join-Path $Target "uninstall.exe"
    foreach ($RequiredPath in @($InstalledBinary, $InstalledUninstaller)) {
        if (-not (Test-Path -LiteralPath $RequiredPath -PathType Leaf)) {
            throw "Installer did not create: $RequiredPath"
        }
    }

    $CargoToml = Get-Content -LiteralPath (Join-Path $ProjectRoot "Cargo.toml") -Raw -Encoding UTF8
    $VersionMatch = [regex]::Match($CargoToml, '(?m)^\s*version\s*=\s*"([^"]+)"')
    if (-not $VersionMatch.Success) {
        throw "Could not read the expected version from Cargo.toml."
    }
    $ExpectedReleaseVersion = $VersionMatch.Groups[1].Value
    $ExpectedVersion = "ask-bridge $ExpectedReleaseVersion"
    $InstalledVersion = (& $InstalledBinary --version).Trim()
    if ($LASTEXITCODE -ne 0 -or $InstalledVersion -ne $ExpectedVersion) {
        throw "Installed version mismatch. Expected '$ExpectedVersion', found '$InstalledVersion'."
    }

    $VersionedFiles = @(
        $Installer,
        $InstalledBinary,
        (Join-Path $Target "ask.exe"),
        (Join-Path $Target "ask-bridge-update.exe"),
        $InstalledUninstaller
    )
    foreach ($VersionedFile in $VersionedFiles) {
        $VersionInfo = (Get-Item -LiteralPath $VersionedFile).VersionInfo
        if ($VersionInfo.FileVersion -ne $ExpectedReleaseVersion -or $VersionInfo.ProductVersion -ne $ExpectedReleaseVersion) {
            throw "Windows version resource mismatch for $VersionedFile. Expected $ExpectedReleaseVersion, found FileVersion=$($VersionInfo.FileVersion) ProductVersion=$($VersionInfo.ProductVersion)."
        }
        if ($VersionInfo.CompanyName -ne "Engels Chou") {
            throw "Windows CompanyName mismatch for $VersionedFile. Expected 'Engels Chou', found '$($VersionInfo.CompanyName)'."
        }
    }

    & $InstalledUninstaller --install-dir $Target --no-path --quiet
    if ($LASTEXITCODE -ne 0) {
        throw "uninstall.exe exited with code $LASTEXITCODE"
    }

    # The uninstaller's detached PowerShell helper can take longer to start on
    # a cold GitHub-hosted runner. Its own bounded retry window is 20 seconds,
    # so allow up to 30 seconds here while still failing on real leftovers.
    for ($Attempt = 0; $Attempt -lt $SelfDeletePollAttempts -and (Test-Path -LiteralPath $Target); $Attempt++) {
        Start-Sleep -Milliseconds $SelfDeletePollMilliseconds
    }
    if (Test-Path -LiteralPath $Target) {
        $Remaining = (Get-ChildItem -LiteralPath $Target -Force | Select-Object -ExpandProperty Name) -join ", "
        throw "Uninstaller did not remove its install directory. Remaining: $Remaining"
    }

    # Reinstall and exercise the direct-launch pause plus overlapping reinstall.
    # The running uninstaller must rename itself to a unique tombstone before it
    # releases the install lock. A new installer may then safely create a fresh
    # uninstall.exe, and the old helper must delete only the tombstone.
    & $Installer --install-dir $Target --no-path --quiet
    if ($LASTEXITCODE -ne 0) {
        throw "install.exe exited with code $LASTEXITCODE during the interactive uninstall test"
    }

    $InstalledUninstaller = Join-Path $Target "uninstall.exe"
    $StartInfo = [Diagnostics.ProcessStartInfo]::new()
    $StartInfo.FileName = $InstalledUninstaller
    $StartInfo.Arguments = "--install-dir `"$Target`" --no-path"
    $StartInfo.UseShellExecute = $false
    $StartInfo.RedirectStandardInput = $true
    $StartInfo.RedirectStandardOutput = $true
    $StartInfo.RedirectStandardError = $true
    $StartInfo.CreateNoWindow = $true

    $UninstallProcess = [Diagnostics.Process]::new()
    $UninstallProcess.StartInfo = $StartInfo
    try {
        if (-not $UninstallProcess.Start()) {
            throw "Could not start the interactive uninstall test."
        }

        Start-Sleep -Milliseconds 1500
        if ($UninstallProcess.HasExited) {
            $EarlyOutput = $UninstallProcess.StandardOutput.ReadToEnd()
            $EarlyError = $UninstallProcess.StandardError.ReadToEnd()
            throw "Interactive uninstaller exited before Enter. Output: $EarlyOutput Error: $EarlyError"
        }
        if (Test-Path -LiteralPath $InstalledUninstaller -PathType Leaf) {
            throw "Interactive uninstaller did not move itself away from the canonical reinstall path."
        }
        $Tombstones = @(Get-ChildItem -LiteralPath $Target -Filter "uninstall.exe.delete-*" -File)
        if ($Tombstones.Count -ne 1) {
            throw "Expected one running-uninstaller tombstone, found $($Tombstones.Count)."
        }

        & $Installer --install-dir $Target --no-path --quiet
        if ($LASTEXITCODE -ne 0) {
            throw "Concurrent reinstall exited with code $LASTEXITCODE"
        }
        if (-not (Test-Path -LiteralPath $InstalledUninstaller -PathType Leaf)) {
            throw "Concurrent reinstall did not create a fresh uninstall.exe."
        }
        $ReinstalledUninstallerHash = (Get-FileHash -LiteralPath $InstalledUninstaller -Algorithm SHA256).Hash

        $UninstallProcess.StandardInput.WriteLine()
        $UninstallProcess.StandardInput.Close()
        if (-not $UninstallProcess.WaitForExit(10000)) {
            $UninstallProcess.Kill()
            throw "Interactive uninstaller did not exit after Enter."
        }
        $InteractiveOutput = $UninstallProcess.StandardOutput.ReadToEnd()
        $InteractiveError = $UninstallProcess.StandardError.ReadToEnd()
        if ($UninstallProcess.ExitCode -ne 0) {
            throw "Interactive uninstaller exited with code $($UninstallProcess.ExitCode). Output: $InteractiveOutput Error: $InteractiveError"
        }
    } finally {
        if (-not $UninstallProcess.HasExited) {
            $UninstallProcess.Kill()
        }
        $UninstallProcess.Dispose()
    }

    for ($Attempt = 0; $Attempt -lt $SelfDeletePollAttempts; $Attempt++) {
        $RemainingTombstones = @(Get-ChildItem -LiteralPath $Target -Filter "uninstall.exe.delete-*" -File -ErrorAction SilentlyContinue)
        if ($RemainingTombstones.Count -eq 0) {
            break
        }
        Start-Sleep -Milliseconds $SelfDeletePollMilliseconds
    }
    $RemainingTombstones = @(Get-ChildItem -LiteralPath $Target -Filter "uninstall.exe.delete-*" -File -ErrorAction SilentlyContinue)
    if ($RemainingTombstones.Count -ne 0) {
        throw "Old interactive uninstaller tombstone was not removed."
    }
    foreach ($ReinstalledPath in @((Join-Path $Target "ask-bridge.exe"), $InstalledUninstaller)) {
        if (-not (Test-Path -LiteralPath $ReinstalledPath -PathType Leaf)) {
            throw "Old self-delete helper removed a newly installed file: $ReinstalledPath"
        }
    }
    if ((Get-FileHash -LiteralPath $InstalledUninstaller -Algorithm SHA256).Hash -ne $ReinstalledUninstallerHash) {
        throw "Old self-delete helper replaced or modified the newly installed uninstaller."
    }

    & $InstalledUninstaller --install-dir $Target --no-path --quiet
    if ($LASTEXITCODE -ne 0) {
        throw "Final cleanup uninstaller exited with code $LASTEXITCODE"
    }
    for ($Attempt = 0; $Attempt -lt $SelfDeletePollAttempts -and (Test-Path -LiteralPath $Target); $Attempt++) {
        Start-Sleep -Milliseconds $SelfDeletePollMilliseconds
    }
    if (Test-Path -LiteralPath $Target) {
        $Remaining = (Get-ChildItem -LiteralPath $Target -Force | Select-Object -ExpandProperty Name) -join ", "
        throw "Final cleanup did not remove the install directory. Remaining: $Remaining"
    }

    Write-Host "Windows installer smoke test passed: $InstalledVersion" -ForegroundColor Green
} finally {
    $ResolvedTarget = [IO.Path]::GetFullPath($Target)
    if ((Test-Path -LiteralPath $ResolvedTarget) -and $ResolvedTarget.StartsWith($SafePrefix, [StringComparison]::OrdinalIgnoreCase)) {
        Remove-Item -LiteralPath $ResolvedTarget -Recurse -Force
    }
}
