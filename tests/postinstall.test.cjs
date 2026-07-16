'use strict';

const assert = require('node:assert/strict');
const { spawnSync } = require('node:child_process');
const { mkdtempSync, readFileSync, writeFileSync } = require('node:fs');
const { tmpdir } = require('node:os');
const { join, resolve } = require('node:path');
const test = require('node:test');

const {
  artifactName,
  cargoTarget,
  platformKey,
  releaseBaseUrl,
  sha256,
  UPDATER_NAME,
  verifyChecksum,
} = require('../npm/postinstall.cjs');

const projectRoot = resolve(__dirname, '..');

function projectFile(path) {
  return readFileSync(join(projectRoot, path), 'utf8');
}

function runMinimumVersionCheck(minimumVersion) {
  const env = {
    ...process.env,
    ASK_BRIDGE_MIN_VERSION: minimumVersion,
    ASK_BRIDGE_VERSION_CHECK_ONLY: '1',
  };

  if (process.platform === 'win32') {
    return spawnSync(
      'powershell',
      [
        '-NoProfile',
        '-ExecutionPolicy',
        'Bypass',
        '-File',
        join(projectRoot, 'install.ps1'),
        '-MinimumVersion',
        minimumVersion,
      ],
      { encoding: 'utf8', env },
    );
  }

  return spawnSync('bash', [join(projectRoot, 'install.sh')], { encoding: 'utf8', env });
}

test('maps supported platforms to Rust targets', () => {
  assert.equal(platformKey('darwin', 'arm64'), 'darwin-arm64');
  assert.equal(cargoTarget('darwin', 'arm64'), 'aarch64-apple-darwin');
  assert.equal(cargoTarget('darwin', 'x64'), 'x86_64-apple-darwin');
  assert.equal(cargoTarget('linux', 'x64'), 'x86_64-unknown-linux-gnu');
  assert.equal(cargoTarget('win32', 'x64'), 'x86_64-pc-windows-msvc');
  assert.equal(UPDATER_NAME, process.platform === 'win32' ? 'ask-bridge-update.exe' : 'ask-bridge-update');
});

test('rejects unsupported platforms', () => {
  assert.throws(() => cargoTarget('linux', 'arm'), /Unsupported platform/);
});

test('formats artifact names and release URLs', () => {
  assert.equal(artifactName('x86_64-unknown-linux-gnu'), 'ask-bridge-x86_64-unknown-linux-gnu.tar.xz');
  assert.equal(artifactName('x86_64-pc-windows-msvc'), 'ask-bridge-x86_64-pc-windows-msvc.zip');
  assert.equal(releaseBaseUrl('1.2.3'), 'https://github.com/EngelsChou/ask-bridge/releases/download/v1.2.3');
});

test('verifies sha256 checksums', () => {
  const dir = mkdtempSync(join(tmpdir(), 'ask-bridge-'));
  const file = join(dir, 'sample.txt');
  writeFileSync(file, 'hello');
  const digest = sha256(file);
  verifyChecksum(file, `${digest}  sample.txt`);
  verifyChecksum(file, `${digest.toUpperCase()} *sample.txt\n`);
  assert.throws(() => verifyChecksum(file, `${'0'.repeat(64)}  sample.txt`), /Checksum mismatch/);
  assert.throws(() => verifyChecksum(file, `${digest}  wrong.txt`), /artifact name/);
  assert.throws(
    () => verifyChecksum(file, `${digest}  sample.txt\n${digest}  appended.txt\n`),
    /Invalid checksum file format/,
  );
});

test('keeps release versions synchronized', () => {
  const version = JSON.parse(projectFile('package.json')).version;
  const escapedVersion = version.replaceAll('.', '\\.');
  const expectedVersions = new Map([
    ['Cargo.toml', new RegExp(`^version = "${escapedVersion}"\\r?$`, 'm')],
    ['Cargo.lock', new RegExp(`name = "ask-bridge"\\r?\\nversion = "${escapedVersion}"`)],
    ['src/main.rs', new RegExp(`#\\[command\\(version = "${escapedVersion}"\\)\\]`)],
    ['install.ps1', new RegExp(`^\\$Version = "${escapedVersion}"\\r?$`, 'm')],
    ['install.sh', new RegExp(`^VERSION="${escapedVersion}"\\r?$`, 'm')],
    ['scripts/ask.sh', new RegExp(`^VERSION="${escapedVersion}"\\r?$`, 'm')],
  ]);

  for (const [path, pattern] of expectedVersions) {
    assert.match(projectFile(path), pattern, `${path} must use release version ${version}`);
  }
});

test('installation and updater entry points never fall back to main', () => {
  const branch = 'main-add-m365-copilot';
  const files = [
    'README.md',
    'README.en.md',
    'public/index.html',
    'src/main.rs',
    'src/update.rs',
    'src/update_policy.rs',
  ];

  for (const path of files) {
    const contents = projectFile(path);
    assert.doesNotMatch(
      contents,
      /raw\.githubusercontent\.com\/EngelsChou\/ask-bridge\/main\//,
      `${path} must not download installer scripts from main`,
    );
  }

  const updatePolicy = projectFile('src/update_policy.rs');
  assert.match(
    updatePolicy,
    /https:\/\/github\.com\/EngelsChou\/ask-bridge\/releases\/latest\/download\/install\.exe/,
  );
  assert.match(updatePolicy, /Get-AuthenticodeSignature/);
  assert.match(updatePolicy, /ASK_BRIDGE_WINDOWS_SIGNER_SHA256/);
  assert.doesNotMatch(updatePolicy, /WINDOWS_INSTALL_SCRIPT_URL/);
  assert.match(
    updatePolicy,
    new RegExp(
      `https://raw\\.githubusercontent\\.com/EngelsChou/ask-bridge/${branch}/install\\.sh`,
    ),
  );

  for (const path of ['src/main.rs', 'src/update.rs']) {
    assert.match(
      projectFile(path),
      /update_policy::(?:windows|unix)_update_command\(env!\("CARGO_PKG_VERSION"\)\)/,
      `${path} must pass its compiled package version to the updater policy`,
    );
  }

  for (const path of ['README.md', 'README.en.md', 'public/index.html']) {
    const contents = projectFile(path);
    assert.doesNotMatch(contents, /npx skills add EngelsChou\/ask-bridge(?:\s|$)/);
    assert.match(contents, new RegExp(`tree/${branch}/skills/ask-bridge`));
    assert.match(contents, new RegExp(`git clone --branch ${branch} --single-branch`));
  }

  const pagesWorkflow = projectFile('.github/workflows/pages.yml');
  assert.match(pagesWorkflow, new RegExp(`branches: \\[${branch}\\]`));
  assert.doesNotMatch(pagesWorkflow, /branches: \[main\]/);
});

test('install scripts verify the fixed release checksum before extraction', () => {
  const powershell = projectFile('install.ps1');
  assert.match(powershell, /\$ChecksumUrl = "\$ReleaseUrl\.sha256"/);
  assert.match(powershell, /Invoke-WebRequest -UseBasicParsing -Uri \$ChecksumUrl -OutFile \$ChecksumPath/);
  assert.match(powershell, /SecurityProtocolType\]::Tls12/);
  assert.match(powershell, /Get-FileHash -LiteralPath \$ZipPath -Algorithm SHA256/);
  assert.ok(
    powershell.indexOf('Get-FileHash -LiteralPath $ZipPath -Algorithm SHA256') <
      powershell.indexOf('Expand-Archive -Path $ZipPath'),
    'PowerShell must verify the archive before extracting it',
  );

  const shell = projectFile('install.sh');
  assert.match(shell, /CHECKSUM_URL="\$\{RELEASE_URL\}\.sha256"/);
  assert.match(shell, /download_file "\$CHECKSUM_URL" "\$CHECKSUM_PATH"/);
  assert.match(shell, /sha256_file\(\)/);
  assert.match(shell, /sha256sum "\$path"/);
  assert.match(shell, /shasum -a 256 "\$path"/);
  assert.match(shell, /ACTUAL_SHA256="\$\(sha256_file "\$ARCHIVE_PATH"\)"/);
  assert.match(shell, /awk 'END \{ print NR \}' "\$CHECKSUM_PATH"/);
  assert.ok(
    shell.indexOf('ACTUAL_SHA256=') < shell.indexOf('tar -xJf "$ARCHIVE_PATH"'),
    'Bash must verify the archive before extracting it',
  );
});

test('install scripts enforce the updater minimum-version floor', () => {
  const version = JSON.parse(projectFile('package.json')).version;
  const major = Number.parseInt(version.split('.')[0], 10);

  for (const acceptedMinimum of ['0.0.0', version]) {
    const accepted = runMinimumVersionCheck(acceptedMinimum);
    assert.ifError(accepted.error);
    assert.equal(
      accepted.status,
      0,
      `expected minimum ${acceptedMinimum} to be accepted:\n${accepted.stdout}${accepted.stderr}`,
    );
  }

  const newerMinimum = `${major + 1}.0.0`;
  const rejected = runMinimumVersionCheck(newerMinimum);
  assert.ifError(rejected.error);
  assert.notEqual(rejected.status, 0, 'an older target release must be rejected');
  assert.match(`${rejected.stdout}${rejected.stderr}`, /Refusing to install ask-bridge/);
});

test('install scripts never execute an existing binary to discover its version', () => {
  const powershell = projectFile('install.ps1');
  assert.doesNotMatch(powershell, /& \$ExistingBinary\s+--version/);
  assert.match(powershell, /VersionInfo\.FileVersion/);

  const shell = projectFile('install.sh');
  assert.doesNotMatch(shell, /\$INSTALL_DIR\/ask-bridge\s+--version/);
  assert.match(shell, /VERSION_RECORD_PATH="\$INSTALL_DIR\/ask-bridge\.version"/);
  assert.match(shell, /RECORDED_BINARY_SHA256/);
  assert.match(shell, /ACTUAL_INSTALLED_SHA256="\$\(sha256_file "\$INSTALL_DIR\/ask-bridge"\)"/);
  assert.match(shell, /printf '%s  %s\\n' "\$VERSION" "\$INSTALLED_BINARY_SHA256"/);
});

test('installers serialize version checks and atomically replace payloads', () => {
  const powershell = projectFile('install.ps1');
  assert.match(powershell, /FileShare\]::None/);
  assert.match(powershell, /ask-bridge-install-.*\[guid\]::NewGuid/);
  assert.match(powershell, /\[System\.IO\.File\]::Replace/);
  assert.ok(
    powershell.indexOf('Enter-AskBridgeInstallLock') < powershell.lastIndexOf('Copy-ItemWithRetry'),
  );

  const shell = projectFile('install.sh');
  assert.match(shell, /\.ask-bridge\.install\.lock\.d/);
  assert.match(shell, /STAGED_BINARY=/);
  assert.match(shell, /mv -f -- "\$STAGED_BINARY" "\$INSTALL_DIR\/ask-bridge"/);
  assert.match(shell, /mv -f -- "\$STAGED_RECORD" "\$VERSION_RECORD_PATH"/);

  const windowsCommon = projectFile('windows-installer/src/common.rs');
  assert.match(windowsCommon, /pub fn acquire_install_lock/);
  assert.match(windowsCommon, /MoveFileExW/);
  assert.doesNotMatch(windowsCommon, /fs::remove_file\(path\)\?;/);
});

test('every package installs the updater before committing the main executable', () => {
  const powershell = projectFile('install.ps1');
  const localUpdater = powershell.indexOf(
    'Copy-ItemWithRetry -Source $ResolvedLocalUpdatePath -Destination $UpdatePath',
  );
  const localAlias = powershell.indexOf(
    'Copy-ItemWithRetry -Source $ResolvedLocalPath -Destination $AliasPath',
  );
  const localMain = powershell.indexOf(
    'Copy-ItemWithRetry -Source $ResolvedLocalPath -Destination $DestPath',
  );
  assert.ok(localUpdater < localAlias && localAlias < localMain);

  const remoteUpdater = powershell.indexOf(
    'Copy-ItemWithRetry -Source $UpdateExePath.FullName -Destination $UpdateDestPath',
  );
  const remoteAlias = powershell.indexOf(
    'Copy-ItemWithRetry -Source $ExePath.FullName -Destination $AliasPath',
  );
  const remoteMain = powershell.indexOf(
    'Copy-ItemWithRetry -Source $ExePath.FullName -Destination $DestPath',
  );
  assert.ok(remoteUpdater < remoteAlias && remoteAlias < remoteMain);

  const offline = projectFile('windows-installer/src/bin/install.rs');
  assert.ok(offline.indexOf('("ask-bridge-update.exe", ASK_BRIDGE_UPDATE)') < offline.indexOf('("ask-bridge.exe", ASK_BRIDGE)'));
  assert.ok(offline.indexOf('("uninstall.exe", UNINSTALLER)') < offline.indexOf('("ask-bridge.exe", ASK_BRIDGE)'));

  const npmPostinstall = projectFile('npm/postinstall.cjs');
  assert.match(npmPostinstall, /findExtractedBinary\(tmpDir, UPDATER_NAME\)/);
  assert.match(npmPostinstall, /verifyWindowsAuthenticode\(extractedUpdater\)/);
  assert.ok(
    npmPostinstall.indexOf('copyFileSync(extractedUpdater, UPDATER_DEST)') <
      npmPostinstall.indexOf('copyFileSync(extracted, DEST)'),
  );
});

test('release publishing supports explicit unsigned fallback and never mutates published assets', () => {
  const workflow = projectFile('.github/workflows/release.yml');
  assert.match(workflow, /ASK_BRIDGE_RELEASE_REQUIRE_SIGNATURE=false/);
  assert.match(workflow, /publishing unsigned Windows executables/);
  assert.match(workflow, /ASK_BRIDGE_RELEASE_REQUIRE_SIGNATURE=true/);
  assert.match(workflow, /publisher must be exactly 'Engels Chou'/);
  assert.doesNotMatch(workflow, /\$path:/);
  assert.match(workflow, /\$\{path\}:/);
  assert.match(workflow, /build-windows-installers\.ps1 -OutputDirectory dist -RequireSignature/);
  assert.match(workflow, /build-windows-installers\.ps1 -OutputDirectory dist\r?\n/);
  assert.ok(
    workflow.indexOf('Build offline Windows installers and payloads') <
      workflow.indexOf('Package binary'),
  );
  assert.match(workflow, /already published; verify it without mutating public assets/);
  assert.match(workflow, /GH_REPO: \$\{\{ github\.repository \}\}/);
  assert.match(workflow, /if: vars\.ASK_BRIDGE_NPM_PUBLISH_ENABLED == 'true'/);
  assert.match(workflow, /gh workflow run npm-publish\.yml --ref "\$TAG"/);
  assert.doesNotMatch(workflow, /apple-darwin|unknown-linux-gnu/);
  assert.match(workflow, /gh run watch "\$run_id" --exit-status/);
});
