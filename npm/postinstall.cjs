#!/usr/bin/env node
'use strict';

const { createHash } = require('node:crypto');
const { spawnSync } = require('node:child_process');
const {
  chmodSync,
  copyFileSync,
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  rmSync,
  writeFileSync,
} = require('node:fs');
const { get } = require('node:https');
const { basename, join } = require('node:path');
const { URL } = require('node:url');

const PACKAGE_ROOT = join(__dirname, '..');
const BINARY_NAME = "ask-bridge";
const GITHUB_OWNER = "EngelsChou";
const GITHUB_REPO = "ask-bridge";
const BIN_DIR = join(__dirname, `${BINARY_NAME}-bin`);
const BIN_NAME = process.platform === 'win32' ? `${BINARY_NAME}.exe` : BINARY_NAME;
const DEST = join(BIN_DIR, BIN_NAME);
const UPDATER_NAME = process.platform === 'win32' ? `${BINARY_NAME}-update.exe` : `${BINARY_NAME}-update`;
const UPDATER_DEST = join(BIN_DIR, UPDATER_NAME);

const TARGETS = {
  'darwin-arm64': 'aarch64-apple-darwin',
  'darwin-x64': 'x86_64-apple-darwin',
  'linux-x64': 'x86_64-unknown-linux-gnu',
  'win32-x64': 'x86_64-pc-windows-msvc',
};

function platformKey(platform = process.platform, arch = process.arch) {
  return `${platform}-${arch}`;
}

function cargoTarget(platform = process.platform, arch = process.arch) {
  const target = TARGETS[platformKey(platform, arch)];
  if (!target) {
    throw new Error(`Unsupported platform: ${platform}/${arch}`);
  }
  return target;
}

function packageVersion() {
  return require(join(PACKAGE_ROOT, 'package.json')).version;
}

function artifactName(target) {
  const ext = target.includes('windows') || target.includes('pc-windows') ? 'zip' : 'tar.xz';
  return `${BINARY_NAME}-${target}.${ext}`;
}

function releaseBaseUrl(version = packageVersion()) {
  return `https://github.com/${GITHUB_OWNER}/${GITHUB_REPO}/releases/download/v${version}`;
}

function sha256(path) {
  return createHash('sha256').update(readFileSync(path)).digest('hex');
}

function verifyChecksum(filePath, checksumText) {
  const normalized = checksumText.replaceAll('\r\n', '\n');
  const lines = normalized.endsWith('\n') ? normalized.slice(0, -1).split('\n') : normalized.split('\n');
  if (lines.length !== 1) {
    throw new Error('Invalid checksum file format');
  }
  const match = lines[0].match(/^([a-fA-F0-9]{64})[ \t]+\*?([^ \t\r\n]+)[ \t]*$/);
  if (!match || match[2] !== basename(filePath)) {
    throw new Error('Invalid checksum file format or artifact name');
  }
  const expected = match[1].toLowerCase();
  const actual = sha256(filePath);
  if (actual !== expected) {
    throw new Error(`Checksum mismatch for ${filePath}: expected ${expected}, got ${actual}`);
  }
}

function download(url, destination, redirectsRemaining = 5) {
  return new Promise((resolve, reject) => {
    get(url, (res) => {
      if (res.statusCode >= 300 && res.statusCode < 400 && res.headers.location && redirectsRemaining > 0) {
        const nextUrl = new URL(res.headers.location, url).toString();
        download(nextUrl, destination, redirectsRemaining - 1).then(resolve, reject);
        return;
      }
      if (res.statusCode !== 200) {
        reject(new Error(`Download failed ${res.statusCode}: ${url}`));
        return;
      }
      const chunks = [];
      res.on('data', (chunk) => chunks.push(chunk));
      res.on('end', () => {
        writeFileSync(destination, Buffer.concat(chunks));
        resolve();
      });
    }).on('error', reject);
  });
}

function run(command, args) {
  const result = spawnSync(command, args, { stdio: 'inherit' });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`Command failed: ${command}`);
}

function extract(archive, destDir) {
  mkdirSync(destDir, { recursive: true });
  if (archive.endsWith('.zip')) {
    if (process.platform === 'win32') {
      run('powershell', ['-NoProfile', '-Command', 'Expand-Archive', '-Force', '-Path', archive, '-DestinationPath', destDir]);
    } else {
      run('unzip', ['-o', archive, '-d', destDir]);
    }
  } else {
    run('tar', ['-xJf', archive, '-C', destDir]);
  }
}

function findExtractedBinary(dir, binName = BIN_NAME) {
  const direct = join(dir, binName);
  if (existsSync(direct)) return direct;

  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    if (!entry.isDirectory()) continue;
    const candidate = join(dir, entry.name, binName);
    if (existsSync(candidate)) return candidate;
  }

  throw new Error(`Archive did not contain ${binName}`);
}

function verifyWindowsAuthenticode(path, expectedVersion = packageVersion()) {
  if (process.platform !== 'win32') return;
  const script = [
    "$ErrorActionPreference='Stop'",
    '$path=$env:ASK_BRIDGE_VERIFY_FILE',
    '$signature=Get-AuthenticodeSignature -LiteralPath $path',
    "if ($signature.Status -ne [System.Management.Automation.SignatureStatus]::Valid -or -not $signature.SignerCertificate) { throw ('Invalid Authenticode signature for ' + $path + ': ' + $signature.Status + ' ' + $signature.StatusMessage) }",
    '$publisher=$signature.SignerCertificate.GetNameInfo([Security.Cryptography.X509Certificates.X509NameType]::SimpleName, $false)',
    "if ($publisher -cne 'Engels Chou') { throw ('Unexpected publisher for ' + $path + ': ' + $publisher) }",
    '$fileVersion=(Get-Item -LiteralPath $path).VersionInfo.FileVersion',
    "if ($fileVersion -notmatch '^\\s*(\\d+)\\.(\\d+)\\.(\\d+)(?:\\.\\d+)?\\s*$') { throw ('Invalid file version for ' + $path + ': ' + $fileVersion) }",
    '$normalizedVersion="$($Matches[1]).$($Matches[2]).$($Matches[3])"',
    "if ($normalizedVersion -cne $env:ASK_BRIDGE_VERIFY_VERSION) { throw ('Unexpected file version for ' + $path + ': ' + $normalizedVersion) }",
  ].join('; ');
  const result = spawnSync('powershell', ['-NoProfile', '-NonInteractive', '-Command', script], {
    encoding: 'utf8',
    env: {
      ...process.env,
      ASK_BRIDGE_VERIFY_FILE: path,
      ASK_BRIDGE_VERIFY_VERSION: expectedVersion,
    },
  });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    throw new Error(`Windows signature verification failed for ${path}: ${result.stderr || result.stdout}`);
  }
}

function installFromLocalBuild() {
  const localRelease = join(PACKAGE_ROOT, 'target', 'release', BIN_NAME);
  const localUpdater = join(PACKAGE_ROOT, 'target', 'release', UPDATER_NAME);
  if (!existsSync(localRelease) || !existsSync(localUpdater)) return false;
  mkdirSync(BIN_DIR, { recursive: true });
  copyFileSync(localUpdater, UPDATER_DEST);
  chmodSync(UPDATER_DEST, 0o755);
  // The main executable is the commit point and is installed last.
  copyFileSync(localRelease, DEST);
  chmodSync(DEST, 0o755);
  return true;
}

async function installFromRelease() {
  const target = cargoTarget();
  const archive = artifactName(target);
  const base = releaseBaseUrl();
  const tmpDir = join(BIN_DIR, '.tmp');
  const archivePath = join(tmpDir, archive);
  const checksumPath = `${archivePath}.sha256`;

  rmSync(tmpDir, { recursive: true, force: true });
  mkdirSync(tmpDir, { recursive: true });
  await download(`${base}/${archive}`, archivePath);
  await download(`${base}/${archive}.sha256`, checksumPath);
  verifyChecksum(archivePath, readFileSync(checksumPath, 'utf8'));
  extract(archivePath, tmpDir);

  const extracted = findExtractedBinary(tmpDir);
  const extractedUpdater = findExtractedBinary(tmpDir, UPDATER_NAME);
  verifyWindowsAuthenticode(extractedUpdater);
  verifyWindowsAuthenticode(extracted);
  mkdirSync(BIN_DIR, { recursive: true });
  copyFileSync(extractedUpdater, UPDATER_DEST);
  chmodSync(UPDATER_DEST, 0o755);
  // The main executable is the commit point and is installed last.
  copyFileSync(extracted, DEST);
  chmodSync(DEST, 0o755);
  rmSync(tmpDir, { recursive: true, force: true });
}

function checkChrome() {
  const chromePaths = {
    darwin: '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
    linux: '/usr/bin/google-chrome',
    win32: 'C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe',
  };
  const chromePath = chromePaths[process.platform];
  if (chromePath && !existsSync(chromePath)) {
    console.warn('');
    console.warn('⚠️  Google Chrome was not found on your system.');
    console.warn('   ask-bridge requires Chrome to automate ChatGPT / Gemini.');
    console.warn('   Please install it from: https://www.google.com/chrome/');
    console.warn('');
  }
}

async function main() {
  if (installFromLocalBuild()) return;
  await installFromRelease();
  checkChrome();
}

if (require.main === module) {
  main().catch((error) => {
    console.error(error.message);
    process.exit(1);
  });
}

module.exports = {
  TARGETS,
  artifactName,
  cargoTarget,
  checkChrome,
  findExtractedBinary,
  platformKey,
  releaseBaseUrl,
  sha256,
  UPDATER_NAME,
  verifyWindowsAuthenticode,
  verifyChecksum,
};
