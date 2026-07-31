#!/usr/bin/env node
// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

/**
 * Assert that references to files actually resolve:
 *   1. every relative import in the deploy scripts,
 *   2. every script path named in the markdown docs, and
 *   3. every security-relevant constant mirrored from Rust into JS/TS.
 *
 * Why this exists: a helper written but never `git add`-ed loads fine on the
 * author's machine and is simply missing for everyone else, so the script dies
 * at import time on a fresh clone. That has happened twice here. CI cannot
 * detect the author's *untracked* files (Actions only sees committed state —
 * that check lives in the pre-commit hook), but it can prove the committed tree
 * is internally consistent, which is how the bug actually surfaces downstream.
 *
 * Lives in a script rather than inline YAML so it can be run and tested
 * locally; the previous inline version used GNU-only `realpath -m` and a regex
 * that silently missed double-quoted and `../` imports, so it was not enforcing
 * its own invariant.
 */

import { execFileSync } from 'child_process';
import { readFileSync } from 'fs';
import { dirname, resolve, relative } from 'path';

const PATTERNS = [
  // `from './x'` / `from "../x"`, `import './x'` (side-effect),
  // `import('./x')` (dynamic), `export ... from './x'`.
  /(?:from|import)\s*\(?\s*['"](\.\.?\/[^'"]+)['"]/g,
];

const tracked = new Set(
  execFileSync('git', ['ls-files'], { encoding: 'utf-8' }).split('\n').filter(Boolean)
);

const files = execFileSync(
  'git',
  ['ls-files', 'deploy/*.mjs', 'deploy/**/*.mjs', 'scripts/*.mjs'],
  { encoding: 'utf-8' }
).split('\n').filter(Boolean);

let checked = 0;
const problems = [];

/**
 * Decide whether a match is real code rather than prose that merely mentions it.
 *
 * Deliberately does NOT strip comments from the source first. A regex-based
 * comment stripper has no string awareness, so any path literal containing `/*`
 * — `'deploy/**''/*.mjs'` in this very file — opens a "comment" that runs to the
 * next `*``/` and deletes the code between, including real imports. That makes the
 * checker silently pass while an unresolved reference exists, which is worse than
 * not having it. Instead, keep the source intact and reject a match whose own
 * line is a comment.
 */
function isCommentLine(src, index) {
  const lineStart = src.lastIndexOf('\n', index) + 1;
  return /^\s*(\/\/|\*|\/\*)/.test(src.slice(lineStart, index));
}

for (const file of files) {
  const src = readFileSync(file, 'utf-8');
  for (const re of PATTERNS) {
    re.lastIndex = 0;
    let m;
    while ((m = re.exec(src)) !== null) {
      if (isCommentLine(src, m.index)) continue;
      const spec = m[1];
      checked++;
      const rel = relative(process.cwd(), resolve(dirname(file), spec));
      if (!tracked.has(rel)) {
        problems.push(`${file} imports ${spec} -> ${rel}, which git does not track`);
      }
    }
  }
}

// ---- 2. script paths named in the docs must exist ----
//
// The deployment guides have repeatedly drifted: recommending a script that was
// renamed, or one that was deleted. Prose is not compiled, so nothing caught it
// until a reviewer read the file. This does.
const DOCS = ['README.md', 'deploy/README.md', 'docs/UPGRADE.md', 'VERIFY.md'];
const SCRIPT_REF = /`?((?:[\w./-]*\/)?[\w-]+\.mjs)`?/g;

for (const doc of DOCS) {
  if (!tracked.has(doc)) continue;
  const text = readFileSync(doc, 'utf-8');
  const seen = new Set();
  let m;
  SCRIPT_REF.lastIndex = 0;
  while ((m = SCRIPT_REF.exec(text)) !== null) {
    const ref = m[1];
    if (seen.has(ref)) continue;
    seen.add(ref);
    checked++;
    // A doc may name a script by bare filename or by repo-relative path; accept
    // either, and resolve a bare name against the usual script directories.
    const candidates = [ref, `deploy/${ref}`, `deploy/helpers/${ref}`, `scripts/${ref}`];
    if (!candidates.some((c) => tracked.has(c))) {
      problems.push(`${doc} refers to ${ref}, which does not exist`);
    }
  }
}

// ---- 3. constants mirrored out of Rust must still match ----
//
// The share-offset bounds decide whether a vault's inflation and burn defences
// hold, and they are hand-copied into the deploy script and the TS test helper.
// Nothing but a comment tied the copies to the Rust source, and drift in the
// permissive direction reintroduces the exact bug the validation exists to stop
// (2-5 SOL spent, then InvalidShareOffset at the final step).
// Every hand-copied constant belongs here, not just the ones that were easiest
// to regex. `MIN_SUPPLY_MULTIPLE` and the generated client's `EXTRA_SHARES` are
// the load-bearing ones: the first decides the minimum first deposit quoted to
// an operator moments before they spend several SOL, and the second is what
// off-chain quotes price against — a client that disagrees with the program
// fails slippage on every attempt.
const RUST_SOURCE = 'programs/august-vault/src/state/vault.rs';
const CLIENT = 'clients/rust/august-vault/src/lib.rs';
const MIRRORS = [
  { file: 'deploy/new-vault.mjs',  konst: 'MIN_SHARE_OFFSET',    js: /const MIN_SHARE_OFFSET = ([\d_]+)/ },
  { file: 'deploy/new-vault.mjs',  konst: 'MAX_SHARE_OFFSET',    js: /const MAX_SHARE_OFFSET = ([\d_]+)/ },
  { file: 'deploy/new-vault.mjs',  konst: 'EXTRA_SHARES',        js: /const DEFAULT_SHARE_OFFSET = ([\d_]+)/ },
  { file: 'deploy/new-vault.mjs',  konst: 'MIN_SUPPLY_MULTIPLE', js: /const MIN_SUPPLY_MULTIPLE = ([\d_]+)/ },
  { file: 'tests/helper/config.ts', konst: 'EXTRA_SHARES',       js: /DEFAULT_SHARE_OFFSET = new anchor\.BN\(([\d_]+)\)/ },
  { file: 'scripts/init-devnet-vault.mjs', konst: 'EXTRA_SHARES', js: /const SHARE_OFFSET = ([\d_]+)/ },
  { file: CLIENT,                  konst: 'EXTRA_SHARES',        js: /pub const EXTRA_SHARES: u128 = ([\d_]+)/ },
  { file: CLIENT,                  konst: 'MIN_SUPPLY_MULTIPLE', js: /pub const MIN_SUPPLY_MULTIPLE: u128 = ([\d_]+)/ },
];

if (tracked.has(RUST_SOURCE)) {
  const rust = readFileSync(RUST_SOURCE, 'utf-8');
  const num = (t) => Number(String(t).replace(/_/g, ''));
  for (const { file, konst, js } of MIRRORS) {
    if (!tracked.has(file)) continue;
    const rustMatch = rust.match(new RegExp(`pub const ${konst}: u128 = ([\\d_]+)`));
    const jsMatch = readFileSync(file, 'utf-8').match(js);
    if (!rustMatch || !jsMatch) {
      problems.push(`could not cross-check ${konst} between ${RUST_SOURCE} and ${file}`);
      continue;
    }
    checked++;
    if (num(rustMatch[1]) !== num(jsMatch[1])) {
      problems.push(
        `${konst} is ${num(rustMatch[1])} in ${RUST_SOURCE} but ${num(jsMatch[1])} in ${file}`
      );
    }
  }
}

if (problems.length > 0) {
  for (const p of problems) console.error(`::error::${p}`);
  console.error(
    `\n${problems.length} unresolved reference(s). Either the file was never ` +
    `staged (git add <file>) or the reference is stale.`
  );
  process.exit(1);
}
console.log(`all ${checked} reference(s) resolve to tracked files`);
