#!/usr/bin/env node
// Promote the "Unreleased" changelog entries into a dated version section and
// keep the Cargo, Cargo.lock, and README versions in sync. The release workflow
// runs this before committing and tagging; it can also be run locally.
import { readFileSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const read = file => readFileSync(join(root, file), 'utf8');
const write = (file, contents) => writeFileSync(join(root, file), contents);

const fail = message => {
  console.error(`prepare-release: ${message}`);
  process.exit(1);
};

const args = process.argv.slice(2);
const version = (args.find(arg => !arg.startsWith('--')) || '').replace(/^v/, '');
if (!/^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/.test(version)) {
  fail('usage: node scripts/prepare-release.mjs <major.minor.patch> [--date YYYY-MM-DD]');
}

const dateIndex = args.indexOf('--date');
const date = dateIndex >= 0 ? args[dateIndex + 1] : new Date().toISOString().slice(0, 10);
if (!/^\d{4}-\d{2}-\d{2}$/.test(date || '')) {
  fail('--date must use YYYY-MM-DD');
}

const parseVersion = value => value.split('-')[0].split('.').map(Number);
const compareVersions = (left, right) => {
  const a = parseVersion(left);
  const b = parseVersion(right);
  for (let index = 0; index < 3; index++) {
    if (a[index] !== b[index]) return a[index] - b[index];
  }
  return 0;
};

const cargo = read('Cargo.toml');
const cargoVersion = cargo.match(/\[package\][\s\S]*?\nversion = "([^"]+)"/);
if (!cargoVersion) fail('could not find the [package] version in Cargo.toml');
const current = cargoVersion[1];
if (compareVersions(version, current) <= 0 && !args.includes('--force')) {
  fail(`version ${version} must be greater than the current version ${current}`);
}

// Build every new file in memory first. A validation failure must leave the
// working tree untouched so a release can be retried safely.
const nextCargo = cargo.replace(
  /(\[package\][\s\S]*?\nversion = ")[^"]+(")/,
  `$1${version}$2`,
);

const lock = read('Cargo.lock');
const nextLock = lock.replace(
  /(\[\[package\]\]\nname = "vaultlite"\nversion = ")[^"]+(")/,
  `$1${version}$2`,
);
if (nextLock === lock) fail('could not find the vaultlite package in Cargo.lock');

const readme = read('README.md');
const nextReadme = readme.replace(/^# VaultLite .*$/m, `# VaultLite ${version}`);
if (nextReadme === readme) fail('could not find the "# VaultLite <version>" README heading');

const changelog = read('CHANGELOG.md');
const lines = changelog.split(/\r?\n/);
const start = lines.findIndex(line => /^## Unreleased\s*$/.test(line));
if (start < 0) fail('CHANGELOG.md is missing a "## Unreleased" section');

let end = start + 1;
while (end < lines.length && !/^#{1,2} /.test(lines[end])) end++;

const entries = lines
  .slice(start + 1, end)
  .join('\n')
  .trim()
  .split('\n')
  .filter(line => line.trim());
if (entries.length === 0) {
  fail('the Unreleased section is empty; add changelog entries before releasing');
}
const escape = value => value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
if (new RegExp(`^## \\[?${escape(version)}\\]?(?:\\s|$)`, 'm').test(changelog)) {
  fail(`CHANGELOG.md already contains a section for ${version}`);
}

const block = ['## Unreleased', '', `## [${version}] - ${date}`, '', ...entries, ''];
const nextChangelog = [...lines.slice(0, start), ...block, ...lines.slice(end)].join('\n');

write('Cargo.toml', nextCargo);
write('Cargo.lock', nextLock);
write('README.md', nextReadme);
write('CHANGELOG.md', nextChangelog);

console.log(`Prepared VaultLite ${version} (${date}).`);
