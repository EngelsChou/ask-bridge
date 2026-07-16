#[cfg(any(target_os = "windows", test))]
const WINDOWS_INSTALLER_URL: &str =
    "https://github.com/EngelsChou/ask-bridge/releases/latest/download/install.exe";
#[cfg(any(target_os = "windows", test))]
const WINDOWS_SIGNER_SHA256: &str = match option_env!("ASK_BRIDGE_WINDOWS_SIGNER_SHA256") {
    Some(value) => value,
    None => "",
};
#[cfg(any(not(target_os = "windows"), test))]
const UNIX_INSTALL_SCRIPT_URL: &str =
    "https://raw.githubusercontent.com/EngelsChou/ask-bridge/main-add-m365-copilot/install.sh";

#[cfg(any(target_os = "windows", test))]
pub fn windows_update_command(current_version: &str) -> String {
    format!(
        concat!(
            "$ErrorActionPreference='Stop'; ",
            "[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12; ",
            "$expectedSignerSha256='{windows_signer_sha256}'; ",
            "if ($expectedSignerSha256 -notmatch '^[0-9a-f]{{64}}$') {{ throw 'This Ask Bridge build has no pinned Engels Chou signing certificate; use the signed offline installer.' }}; ",
            "$minimumVersion=[version]'{current_version}'; ",
            "$installerPath=Join-Path $env:TEMP ('ask-bridge-update-' + [guid]::NewGuid().ToString('N') + '.exe'); ",
            "try {{ ",
            "Invoke-WebRequest -UseBasicParsing -Uri '{windows_installer_url}' -OutFile $installerPath; ",
            "$signature=Get-AuthenticodeSignature -LiteralPath $installerPath; ",
            "if ($signature.Status -ne [System.Management.Automation.SignatureStatus]::Valid -or -not $signature.SignerCertificate) {{ throw ('Downloaded installer Authenticode verification failed: ' + $signature.Status + ' ' + $signature.StatusMessage) }}; ",
            "$publisher=$signature.SignerCertificate.GetNameInfo([Security.Cryptography.X509Certificates.X509NameType]::SimpleName, $false); ",
            "if ($publisher -cne 'Engels Chou') {{ throw ('Unexpected installer publisher: ' + $publisher) }}; ",
            "$sha256=[Security.Cryptography.SHA256]::Create(); ",
            "try {{ $actualSignerSha256=([BitConverter]::ToString($sha256.ComputeHash($signature.SignerCertificate.RawData))).Replace('-', '').ToLowerInvariant() }} finally {{ $sha256.Dispose() }}; ",
            "if ($actualSignerSha256 -cne $expectedSignerSha256) {{ throw 'Downloaded installer signer certificate does not match the pinned Engels Chou certificate.' }}; ",
            "$fileVersion=(Get-Item -LiteralPath $installerPath).VersionInfo.FileVersion; ",
            "if ($fileVersion -notmatch '^\\s*(\\d+)\\.(\\d+)\\.(\\d+)(?:\\.\\d+)?\\s*$') {{ throw ('Downloaded installer has an invalid file version: ' + $fileVersion) }}; ",
            "$targetVersion=[version](\"$($Matches[1]).$($Matches[2]).$($Matches[3])\"); ",
            "if ($targetVersion.CompareTo($minimumVersion) -lt 0) {{ throw (\"Refusing to downgrade Ask Bridge from $minimumVersion to $targetVersion.\") }}; ",
            "$process=Start-Process -FilePath $installerPath -ArgumentList @('--quiet') -Wait -PassThru; ",
            "if ($process.ExitCode -ne 0) {{ throw ('Signed Ask Bridge installer failed with exit code ' + $process.ExitCode) }} ",
            "}} finally {{ Remove-Item -LiteralPath $installerPath -Force -ErrorAction SilentlyContinue }}"
        ),
        current_version = current_version,
        windows_signer_sha256 = WINDOWS_SIGNER_SHA256,
        windows_installer_url = WINDOWS_INSTALLER_URL,
    )
}

#[cfg(any(target_os = "windows", test))]
#[allow(dead_code)] // Included by both binaries; only the main CLI detaches from its own PID.
pub fn windows_update_command_after_parent(current_version: &str, parent_pid: u32) -> String {
    let update = windows_update_command(current_version);
    format!(
        "$ErrorActionPreference='Stop'; $parentProcess=Get-Process -Id {parent_pid} -ErrorAction SilentlyContinue; if ($parentProcess) {{ $parentProcess.WaitForExit() }}; {update}"
    )
}

#[cfg(any(not(target_os = "windows"), test))]
pub fn unix_update_command(current_version: &str) -> String {
    format!(
        "set -o pipefail; curl -fsSL '{UNIX_INSTALL_SCRIPT_URL}' | ASK_BRIDGE_MIN_VERSION='{current_version}' bash"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn updater_uses_signed_windows_release_and_fixed_unix_branch() {
        assert_eq!(
            WINDOWS_INSTALLER_URL,
            "https://github.com/EngelsChou/ask-bridge/releases/latest/download/install.exe"
        );
        assert_eq!(
            UNIX_INSTALL_SCRIPT_URL,
            "https://raw.githubusercontent.com/EngelsChou/ask-bridge/main-add-m365-copilot/install.sh"
        );
        assert!(!WINDOWS_INSTALLER_URL.contains("raw.githubusercontent.com"));
        assert!(!UNIX_INSTALL_SCRIPT_URL.contains("/main/"));
    }

    #[test]
    fn updater_passes_its_current_version_as_the_downgrade_floor() {
        let windows = windows_update_command("0.3.1");
        assert!(windows.contains("$minimumVersion=[version]'0.3.1'"));
        assert!(windows.contains("Get-AuthenticodeSignature"));
        assert!(windows.contains("Engels Chou"));
        assert!(windows.contains("Tls12"));
        assert!(windows.contains(WINDOWS_INSTALLER_URL));
        assert!(!windows.contains("Invoke-Expression"));

        let detached = windows_update_command_after_parent("0.3.1", 4242);
        assert!(detached.contains("Get-Process -Id 4242"));
        assert!(detached.contains("$parentProcess.WaitForExit()"));
        assert!(detached.contains(WINDOWS_INSTALLER_URL));

        let unix = unix_update_command("0.3.1");
        assert!(unix.contains("ASK_BRIDGE_MIN_VERSION='0.3.1' bash"));
        assert!(unix.starts_with("set -o pipefail;"));
    }
}
