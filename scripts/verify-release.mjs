#!/usr/bin/env node
// Fail fast when a tag does not match the committed version and changelog.
// The release workflow runs this before spending time on builds.
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const tag = (process.argv[2] || process.env.GITHUB_REF_NAME || '').trim();
const version = tag.replace(/^v/, '');

const fail = message => {
  console.error(`verify-release: ${message}`);
  process.exit(1);
};

if (!/^v?\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/.test(tag)) {
  fail(`"${tag}" is not a semantic version tag`);
}
if (!tag.startsWith('v')) fail('release tags must start with "v"');

const cargo = readFileSync(join(root, 'Cargo.toml'), 'utf8');
const match = cargo.match(/\[package\][\s\S]*?\nversion = "([^"]+)"/);
if (!match) fail('could not read the [package] version from Cargo.toml');
if (match[1] !== version) {
  fail(`Cargo.toml has ${match[1]}, but the tag is ${tag}`);
}

const readme = readFileSync(join(root, 'README.md'), 'utf8');
if (!readme.includes(`# VaultLite ${version}`)) {
  fail(`README.md does not mention VaultLite ${version}`);
}

const changelog = readFileSync(join(root, 'CHANGELOG.md'), 'utf8');
const escape = value => value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
if (!new RegExp(`^## \\[?${escape(version)}\\]?(?:\\s|$)`, 'm').test(changelog)) {
  fail(`CHANGELOG.md has no section for ${version}`);
}

console.log(`Verified ${tag}: Cargo.toml, README.md, and CHANGELOG.md agree.`);
