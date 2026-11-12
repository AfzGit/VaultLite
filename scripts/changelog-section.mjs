#!/usr/bin/env node
// Print the changelog body for one version so a GitHub Release can use the
// hand-written notes instead of GitHub's generated commit list.
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const tag = (process.argv[2] || '').trim().replace(/^v/, '');

if (!tag) {
  console.error('usage: node scripts/changelog-section.mjs <version|vversion>');
  process.exit(1);
}

const escape = value => value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
const lines = readFileSync(join(root, 'CHANGELOG.md'), 'utf8').split(/\r?\n/);
const heading = new RegExp(`^## \\[?${escape(tag)}\\]?(?:\\s|$)`);
const start = lines.findIndex(line => heading.test(line));

if (start < 0) {
  console.error(`changelog-section: CHANGELOG.md has no section for ${tag}`);
  process.exit(1);
}

let end = start + 1;
while (end < lines.length && !/^#{1,2} /.test(lines[end])) end++;

const body = lines.slice(start + 1, end).join('\n').trim();
if (!body) {
  console.error(`changelog-section: the section for ${tag} is empty`);
  process.exit(1);
}

process.stdout.write(`${body}\n`);
