[CmdletBinding()]
param(
    [string] $OutputDirectory = "dist\windows"
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

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
    $ExpectedVersion = "ask-bridge $($VersionMatch.Groups[1].Value)"
    $InstalledVersion = (& $InstalledBinary --version).Trim()
    if ($LASTEXITCODE -ne 0 -or $InstalledVersion -ne $ExpectedVersion) {
        throw "Installed version mismatch. Expected '$ExpectedVersion', found '$InstalledVersion'."
    }

    & $InstalledUninstaller --install-dir $Target --no-path --quiet
    if ($LASTEXITCODE -ne 0) {
        throw "uninstall.exe exited with code $LASTEXITCODE"
    }

    for ($Attempt = 0; $Attempt -lt 20 -and (Test-Path -LiteralPath $Target); $Attempt++) {
        Start-Sleep -Milliseconds 250
    }
    if (Test-Path -LiteralPath $Target) {
        $Remaining = (Get-ChildItem -LiteralPath $Target -Force | Select-Object -ExpandProperty Name) -join ", "
        throw "Uninstaller did not remove its install directory. Remaining: $Remaining"
    }

    # Reinstall and exercise the direct-launch pause path. The self-delete helper
    # must keep waiting while uninstall.exe is locked, then remove it after Enter.
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
        if (-not (Test-Path -LiteralPath $InstalledUninstaller -PathType Leaf)) {
            throw "Interactive uninstaller was deleted while it was still running."
        }

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

    for ($Attempt = 0; $Attempt -lt 40 -and (Test-Path -LiteralPath $Target); $Attempt++) {
        Start-Sleep -Milliseconds 250
    }
    if (Test-Path -LiteralPath $Target) {
        $Remaining = (Get-ChildItem -LiteralPath $Target -Force | Select-Object -ExpandProperty Name) -join ", "
        throw "Interactive uninstaller did not remove its install directory. Remaining: $Remaining"
    }

    Write-Host "Windows installer smoke test passed: $InstalledVersion" -ForegroundColor Green
} finally {
    $ResolvedTarget = [IO.Path]::GetFullPath($Target)
    if ((Test-Path -LiteralPath $ResolvedTarget) -and $ResolvedTarget.StartsWith($SafePrefix, [StringComparison]::OrdinalIgnoreCase)) {
        Remove-Item -LiteralPath $ResolvedTarget -Recurse -Force
    }
}
