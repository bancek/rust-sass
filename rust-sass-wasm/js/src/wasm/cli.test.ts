// CLI end-to-end + differential tests. Spawns the compiled `dist/bin/sass.js`
// and, for parity cases, the official `sass` npm package's CLI, comparing
// stdout/stderr/exit codes byte-for-byte. Requires `npm run build:js` (and the
// wasm artifacts).

import * as child from 'node:child_process';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as p from 'node:path';

import { afterAll, describe, expect, it } from 'vitest';

import { artifactsPresent, tempDir } from './helpers';

const bin = p.resolve(__dirname, '..', '..', 'dist', 'bin', 'sass.js');
const sassBin = p.resolve(
  __dirname,
  '..',
  '..',
  '..',
  'node_modules',
  'sass',
  'sass.js',
);
const distPresent = fs.existsSync(bin);

const tempDirs: string[] = [];
function fixtureDir(): string {
  const dir = tempDir();
  tempDirs.push(dir);
  return dir;
}

afterAll(() => {
  for (const dir of tempDirs) fs.rmSync(dir, { recursive: true, force: true });
});

interface RunResult {
  stdout: string;
  stderr: string;
  status: number;
}

function run(
  binPath: string,
  args: string[],
  opts: { cwd?: string; stdin?: string } = {},
): RunResult {
  const r = child.spawnSync(process.execPath, [binPath, ...args], {
    cwd: opts.cwd,
    input: opts.stdin,
    encoding: 'utf8',
    stdio: ['pipe', 'pipe', 'pipe'],
  });
  return {
    stdout: r.stdout ?? '',
    stderr: r.stderr ?? '',
    status: r.status ?? -1,
  };
}

function ours(
  args: string[],
  opts: { cwd?: string; stdin?: string } = {},
): RunResult {
  return run(bin, args, opts);
}

/** Runs both CLIs with identical args/cwd/stdin and asserts equality. */
function assertParity(
  args: string[],
  opts: { cwd?: string; stdin?: string } = {},
): void {
  const reference = run(sassBin, args, opts);
  const actual = ours(args, opts);
  expect(actual.stdout).toBe(reference.stdout);
  expect(actual.stderr).toBe(reference.stderr);
  expect(actual.status).toBe(reference.status);
}

describe.skipIf(!artifactsPresent || !distPresent)('cli end-to-end', () => {
  it('prints the version', () => {
    const r = ours(['--version']);
    expect(r.status).toBe(0);
    expect(r.stdout).toBe('1.104.0 compiled with dart2js 3.13.3\n');
  });

  it('compiles stdin to stdout', () => {
    const r = ours(['--stdin', '--no-color', '--no-unicode'], {
      stdin: 'a { b: c; }\n',
    });
    expect(r.status).toBe(0);
    expect(r.stdout).toBe('a {\n  b: c;\n}\n');
  });

  it('captures @warn on stderr', () => {
    const r = ours(['--stdin', '--no-color', '--no-unicode'], {
      stdin: '@warn "hi";\na{b:c}\n',
    });
    expect(r.status).toBe(0);
    expect(r.stderr).toBe('WARNING: hi\n    - 1:1  root stylesheet\n\n');
  });

  it('exits 65 for @error with the rendered error on stderr', () => {
    const r = ours(['--stdin', '--no-color', '--no-unicode'], {
      stdin: '@error "boom";\n',
    });
    expect(r.status).toBe(65);
    expect(r.stderr).toBe(
      'Error: "boom"\n  ,\n1 | @error "boom";\n  | ^^^^^^^^^^^^^\n  \'\n  - 1:1  root stylesheet\n',
    );
  });

  it('exits 65 for a syntax error', () => {
    const r = ours(['--stdin', '--no-color', '--no-unicode'], {
      stdin: 'a { b: ; }\n',
    });
    expect(r.status).toBe(65);
    expect(r.stderr).toBe(
      "Error: Expected expression.\n  ,\n1 | a { b: ; }\n  |        ^\n  '\n  - 1:8  root stylesheet\n",
    );
  });

  it('--quiet suppresses warnings', () => {
    const r = ours(['--stdin', '--quiet', '--no-color', '--no-unicode'], {
      stdin: '@warn "hi";\na{b:c}\n',
    });
    expect(r.status).toBe(0);
    expect(r.stderr).toBe('');
  });

  it('--indented reads indented syntax from stdin', () => {
    const r = ours(['--stdin', '--indented', '--no-color', '--no-unicode'], {
      stdin: 'a\n  b: c\n',
    });
    expect(r.status).toBe(0);
    expect(r.stdout).toBe('a {\n  b: c;\n}\n');
  });

  it('compiles a file to a destination with no success line', () => {
    const dir = fixtureDir();
    fs.writeFileSync(p.join(dir, 'input.scss'), 'a { b: c; }\n');
    const r = ours(['input.scss', 'out.css', '--no-source-map', '--no-color'], {
      cwd: dir,
    });
    expect(r.status).toBe(0);
    expect(fs.readFileSync(p.join(dir, 'out.css'), 'utf8')).toBe(
      'a {\n  b: c;\n}\n',
    );
    // The official sass.js CLI prints no "Compiled X to Y." line (1.100.0).
    expect(r.stdout).toBe('');
  });

  it('exits 66 for a missing input file', () => {
    const dir = fixtureDir();
    const r = ours(['missing.scss', '--no-color'], { cwd: dir });
    expect(r.status).toBe(66);
    expect(r.stderr).toBe(
      'Error reading missing.scss: no such file or directory.\n',
    );
  });

  it('exits 64 for usage errors', () => {
    expect(ours([], { cwd: fixtureDir() }).status).toBe(64);
    expect(ours(['--bogus'], { cwd: fixtureDir() }).status).toBe(64);
  });
});

describe.skipIf(!artifactsPresent || !distPresent)(
  'cli differential with sass',
  () => {
    it('stdin basic compile', () => {
      assertParity(['--stdin', '--no-color', '--no-unicode'], {
        stdin: 'a { b: c; }\n',
      });
    });

    it('stdin warnings and debug', () => {
      assertParity(['--stdin', '--no-color', '--no-unicode'], {
        stdin: '@warn "hello";\n@debug 42;\na { b: c; }\n',
      });
    });

    it('stdin @error', () => {
      assertParity(['--stdin', '--no-color', '--no-unicode'], {
        stdin: '@error "boom";\n',
      });
    });

    it('stdin syntax error', () => {
      assertParity(['--stdin', '--no-color', '--no-unicode'], {
        stdin: 'a { b: ; }\n',
      });
    });

    it('stdin --indented', () => {
      assertParity(['--stdin', '--indented', '--no-color', '--no-unicode'], {
        stdin: 'a\n  b: c\n',
      });
    });

    it('dash positional reads stdin', () => {
      assertParity(['-', '--no-color', '--no-unicode'], {
        stdin: 'a { b: c; }\n',
      });
    });

    it('file to stdout', () => {
      const dir = fixtureDir();
      fs.writeFileSync(p.join(dir, 'input.scss'), '$c: red; a { b: $c; }\n');
      assertParity(['input.scss', '--no-color', '--no-unicode'], { cwd: dir });
    });

    it('file to stdout compressed', () => {
      const dir = fixtureDir();
      fs.writeFileSync(p.join(dir, 'input.scss'), '$c: red; a { b: $c; }\n');
      assertParity(
        ['input.scss', '--style', 'compressed', '--no-color', '--no-unicode'],
        { cwd: dir },
      );
    });

    it('invalid UTF-8 input file', () => {
      const dir = fixtureDir();
      // "$" followed by invalid bytes (spec/libsass-closed-issues/issue_2446).
      fs.writeFileSync(
        p.join(dir, 'input.scss'),
        Buffer.from([
          0x24, 0xff, 0x3a, 0x44, 0x26, 0x28, 0x32, 0x32, 0x23, 0x32,
        ]),
      );
      assertParity(['input.scss', '--no-color', '--no-unicode'], { cwd: dir });
    });

    it('file with dest and source map', () => {
      const dir = fixtureDir();
      fs.writeFileSync(p.join(dir, 'input.scss'), 'a { b: c; }\n');
      assertParity(['input.scss', 'out.css', '--no-color', '--no-unicode'], {
        cwd: dir,
      });
    });

    it('stdin with embedded source map', () => {
      assertParity(
        [
          '--stdin',
          '--source-map',
          '--embed-source-map',
          '--no-color',
          '--no-unicode',
        ],
        { stdin: 'a { b: c; }\n' },
      );
    });

    it('file with --embed-sources', () => {
      const dir = fixtureDir();
      fs.writeFileSync(p.join(dir, 'input.scss'), 'a { b: c; }\n');
      assertParity(
        [
          'input.scss',
          'out.css',
          '--embed-sources',
          '--no-color',
          '--no-unicode',
        ],
        { cwd: dir },
      );
    });

    it('load path resolution', () => {
      const dir = fixtureDir();
      fs.mkdirSync(p.join(dir, 'lib'));
      fs.writeFileSync(p.join(dir, 'lib', '_vars.scss'), '$c: red;\n');
      fs.writeFileSync(
        p.join(dir, 'input.scss'),
        '@use "vars"; a { b: $c; }\n',
      );
      assertParity(['input.scss', '-I', 'lib', '--no-color', '--no-unicode'], {
        cwd: dir,
      });
    });

    it('--quiet suppresses warnings', () => {
      assertParity(['--stdin', '--quiet', '--no-color', '--no-unicode'], {
        stdin: '@warn "hi";\na{b:c}\n',
      });
    });

    it('--fatal-deprecation turns a deprecation into an error', () => {
      assertParity(
        [
          '--stdin',
          '--fatal-deprecation',
          'new-global',
          '--no-color',
          '--no-unicode',
        ],
        { stdin: 'a { $b: c !global; }\n' },
      );
    });

    it('missing input exits 66', () => {
      const dir = fixtureDir();
      assertParity(['missing.scss', '--no-color', '--no-unicode'], {
        cwd: dir,
      });
    });

    it('directory mode', () => {
      const dir = fixtureDir();
      const src = p.join(dir, 'src');
      const out = p.join(dir, 'out');
      fs.mkdirSync(p.join(src, 'sub'), { recursive: true });
      fs.writeFileSync(p.join(src, 'a.scss'), 'a { b: c; }\n');
      fs.writeFileSync(p.join(src, '_partial.scss'), 'x { y: z; }\n');
      fs.writeFileSync(p.join(src, 'sub', 'b.scss'), 'b { c: d; }\n');
      assertParity(['src:out', '--no-color', '--no-unicode'], { cwd: dir });
    });
  },
);
