#!/usr/bin/env node
/**
 * Compile the two SMUI themes — but only when they are actually stale, and
 * without the wall of Sass deprecation warnings.
 *
 * `smui-theme` calls Dart Sass's legacy `renderSync` with no logger hook, so
 * the deprecation notices MDC's own stylesheets provoke cannot be silenced
 * with a flag: they are filtered here instead. Real errors are never
 * swallowed — a failing compile prints everything it said and exits non-zero.
 *
 * The outputs are gitignored, so a fresh checkout compiles once; after that
 * `npm install` (which runs this through the `prepare` hook) says nothing and
 * costs nothing. Pass --force to compile regardless.
 */

import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync, readdirSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const uiDir = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const force = process.argv.includes('--force');

const TARGETS = [
    { out: join(uiDir, 'public', 'smui.css'), includes: join(uiDir, 'src', 'theme') },
    { out: join(uiDir, 'public', 'smui-dark.css'), includes: join(uiDir, 'src', 'theme', 'dark') },
];
/** Gitignored alongside the CSS it describes. */
const STAMP = join(uiDir, 'public', '.smui-theme-stamp');

/**
 * What the compiled CSS actually depends on: the theme sources and the
 * versions of the packages whose SCSS is pulled in. Hashed rather than
 * mtime-compared because `npm install` rewrites the lockfile on every run,
 * which would make a timestamp check recompile forever.
 */
function inputHash() {
    const h = createHash('sha256');
    const walk = (dir) => {
        for (const entry of readdirSync(dir, { withFileTypes: true }).sort((a, b) => a.name.localeCompare(b.name))) {
            const p = join(dir, entry.name);
            if (entry.isDirectory()) walk(p);
            else h.update(entry.name).update(readFileSync(p));
        }
    };
    walk(join(uiDir, 'src', 'theme'));
    const pkg = JSON.parse(readFileSync(join(uiDir, 'package.json'), 'utf8'));
    h.update(JSON.stringify({ ...pkg.dependencies, ...pkg.devDependencies }));
    for (const dep of ['smui-theme', '@material/theme']) {
        const p = join(uiDir, 'node_modules', dep, 'package.json');
        if (existsSync(p)) h.update(JSON.parse(readFileSync(p, 'utf8')).version ?? '');
    }
    return h.digest('hex');
}

function isFresh(hash) {
    if (!TARGETS.every((t) => existsSync(t.out)) || !existsSync(STAMP)) return false;
    return readFileSync(STAMP, 'utf8').trim() === hash;
}

/**
 * A successful compile has nothing to say: everything Sass prints is a
 * deprecation notice about MDC's own stylesheets, in either the multi-line
 * or the compact form. So on success keep only lines that mention an error —
 * cheap insurance against a non-fatal problem going unseen — and on failure
 * (handled by the caller) print the lot.
 */
function interesting(text) {
    return text
        .split('\n')
        .filter((line) => /error/i.test(line))
        .join('\n')
        .trim();
}

const hash = inputHash();
if (!force && isFresh(hash)) process.exit(0);

process.stdout.write('Compiling SMUI themes…\n');
for (const { out, includes } of TARGETS) {
    const bin = join(uiDir, 'node_modules', '.bin', 'smui-theme');
    const r = spawnSync(bin, ['compile', out, '-i', includes], {
        cwd: uiDir,
        encoding: 'utf8',
        env: { ...process.env, NODE_OPTIONS: `${process.env.NODE_OPTIONS ?? ''} --no-deprecation`.trim() },
    });
    if (r.status !== 0) {
        // Failure: hand back everything, unfiltered.
        process.stdout.write(r.stdout ?? '');
        process.stderr.write(r.stderr ?? '');
        process.stderr.write(`\nsmui-theme failed for ${out}\n`);
        process.exit(r.status ?? 1);
    }
    const rest = interesting(`${r.stdout ?? ''}\n${r.stderr ?? ''}`);
    if (rest) process.stderr.write(`${rest}\n`);
}
writeFileSync(STAMP, `${hash}\n`);
