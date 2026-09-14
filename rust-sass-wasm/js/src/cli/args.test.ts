import * as fs from 'node:fs';
import * as os from 'node:os';
import * as p from 'node:path';

import { afterAll, describe, expect, it } from 'vitest';

import {
  CliOptions,
  UsageError,
  parseArgs,
  relativePath,
  splitSourceAndDestination,
} from './args';

function parse(argv: string[]): CliOptions {
  return parseArgs(argv, false);
}

function parseErr(argv: string[]): string {
  try {
    parse(argv);
    return '';
  } catch (e) {
    if (e instanceof UsageError) return e.message;
    throw e;
  }
}

const tempDirs: string[] = [];
function tempDir(): string {
  const dir = fs.mkdtempSync(p.join(os.tmpdir(), 'sass-cli-'));
  tempDirs.push(dir);
  return dir;
}

afterAll(() => {
  for (const dir of tempDirs) fs.rmSync(dir, { recursive: true, force: true });
});

describe('parseArgs', () => {
  it('no args is a usage error', () => {
    expect(parseErr([])).toBe('Compile Sass to CSS.');
  });

  it('single file to stdout', () => {
    const o = parse(['input.scss']);
    expect(o.sources).toEqual([{ source: 'input.scss', dest: null }]);
    expect(o.emitSourceMap).toBe(false); // stdout without --embed-source-map
  });

  it('two positionals', () => {
    const o = parse(['in.scss', 'out.css']);
    expect(o.sources).toEqual([{ source: 'in.scss', dest: 'out.css' }]);
  });

  it('stdin flag with output', () => {
    const o = parse(['--stdin', 'out.css']);
    expect(o.sources).toEqual([{ source: null, dest: 'out.css' }]);
  });

  it('dash is stdin', () => {
    const o = parse(['-', 'out.css']);
    expect(o.sources).toEqual([{ source: null, dest: 'out.css' }]);
  });

  it('colon form', () => {
    const o = parse(['in.scss:out.css']);
    expect(o.sources).toEqual([{ source: 'in.scss', dest: 'out.css' }]);
  });

  it('too many positionals', () => {
    expect(parseErr(['a', 'b', 'c'])).toBe(
      'Only two positional args may be passed.',
    );
  });

  it('mixing positional and colon args', () => {
    expect(parseErr(['a', 'bc:d'])).toBe(
      'Positional and ":" arguments may not both be used.',
    );
  });

  it('double colon', () => {
    expect(parseErr(['in.scss:out:css'])).toBe(
      '"in.scss:out:css" may only contain one ":".',
    );
  });

  it('windows drive letter colon is not a separator', () => {
    const o = parse(['C:foo.scss']);
    expect(o.sources).toEqual([{ source: 'C:foo.scss', dest: null }]);
  });

  it('stdin too many args', () => {
    expect(parseErr(['--stdin', 'a', 'b'])).toBe(
      'Only one argument is allowed with --stdin.',
    );
  });

  it('stdin with colon arg', () => {
    expect(parseErr(['--stdin', 'ab:c'])).toBe(
      '--stdin may not be used with ":" arguments.',
    );
  });

  it('empty positional', () => {
    expect(parseErr([''])).toBe('Invalid argument "".');
  });

  it('no source map blocks map flags', () => {
    expect(parseErr(['--no-source-map', '--embed-sources', 'a.scss'])).toBe(
      "--embed-sources isn't allowed with --no-source-map.",
    );
    expect(
      parseErr(['--no-source-map', '--source-map-urls=absolute', 'a.scss']),
    ).toBe("--source-map-urls isn't allowed with --no-source-map.");
    expect(parseErr(['--no-source-map', '--embed-source-map', 'a.scss'])).toBe(
      "--embed-source-map isn't allowed with --no-source-map.",
    );
  });

  it('stdout requires embed source map', () => {
    expect(parseErr(['--source-map', 'a.scss'])).toBe(
      'When printing to stdout, --source-map requires --embed-source-map.',
    );
    expect(
      parseErr(['--source-map', '--embed-source-map', 'a.scss']).length,
    ).toBe(0);
  });

  it('stdout relative urls blocked', () => {
    expect(parseErr(['--source-map-urls=relative', 'a.scss'])).toBe(
      "--source-map-urls=relative isn't allowed when printing to stdout.",
    );
  });

  it('source maps default off for stdout', () => {
    expect(parse(['a.scss']).emitSourceMap).toBe(false);
    expect(parse(['--embed-source-map', 'a.scss']).emitSourceMap).toBe(true);
    expect(parseErr(['--embed-sources', 'a.scss'])).toBe(
      'When printing to stdout, --embed-sources requires --embed-source-map.',
    );
  });

  it('source maps on for file destinations', () => {
    expect(parse(['a.scss', 'out.css']).emitSourceMap).toBe(true);
    expect(parse(['--no-source-map', 'a.scss', 'out.css']).emitSourceMap).toBe(
      false,
    );
  });

  it('invalid deprecation', () => {
    expect(parseErr(['--silence-deprecation=nope', 'a.scss'])).toBe(
      'Invalid deprecation "nope".',
    );
    expect(parseErr(['--future-deprecation=nope', 'a.scss'])).toBe(
      'Invalid deprecation "nope".',
    );
    expect(parseErr(['--fatal-deprecation=nope', 'a.scss'])).toBe(
      'Invalid deprecation "nope".',
    );
  });

  it('fatal deprecation version resolution', () => {
    const o = parse(['--fatal-deprecation', 'new-global', 'a.scss']);
    expect(o.fatalDeprecations).toEqual(['new-global']);
    const v = parse(['--fatal-deprecation', '1.17.2', 'a.scss']);
    expect(v.fatalDeprecations[0].toString()).toBe('1.17.2');
  });

  it('fatal deprecation version above current is rejected', () => {
    expect(parseErr(['--fatal-deprecation', '9.9.9', 'a.scss'])).toBe(
      'Invalid version 9.9.9. --fatal-deprecation requires a version less than or equal to the current Dart Sass version.',
    );
  });

  it('style and charset', () => {
    const o = parse(['--style', 'compressed', '--no-charset', 'a.scss']);
    expect(o.style).toBe('compressed');
    expect(o.charset).toBe(false);
    expect(parse(['a.scss']).style).toBe('expanded');
    expect(parse(['a.scss']).charset).toBe(true);
  });

  it('precision and async are accepted', () => {
    const o = parse(['--precision', '10', '--async', 'a.scss']);
    expect(o.sources.length).toBe(1);
  });

  it('negatable flags last one wins', () => {
    expect(parse(['--no-color', '--color', 'a.scss']).alertColor).toBe(true);
    expect(parse(['--color', '--no-color', 'a.scss']).alertColor).toBe(false);
    expect(parse(['a.scss']).alertColor).toBe(false); // no tty in tests
    expect(parse(['--no-unicode', 'a.scss']).unicode).toBe(false);
    expect(parse(['--unicode', '--no-unicode', 'a.scss']).unicode).toBe(false);
  });

  it('short flag forms', () => {
    const o1 = parse(['-s', 'compressed', '-I', 'lib', 'a.scss']);
    expect(o1.style).toBe('compressed');
    expect(o1.loadPaths).toEqual(['lib']);
    const o2 = parse(['-scompressed', '-Ilib', '-I=other', 'a.scss']);
    expect(o2.style).toBe('compressed');
    expect(o2.loadPaths).toEqual(['lib', 'other']);
    expect(parse(['-q', 'a.scss']).silent).toBe(true);
  });

  it('pkg-importer value validation', () => {
    expect(
      parse(['--pkg-importer', 'node', 'a.scss']).nodePackageImporter,
    ).toBe(true);
    expect(parseErr(['--pkg-importer', 'npm', 'a.scss'])).toBe(
      'Invalid value "npm" for --pkg-importer.',
    );
  });

  it('unknown flags are usage errors', () => {
    expect(parseErr(['--bogus', 'a.scss'])).toBe(
      'Could not find an option named "--bogus".',
    );
    expect(parseErr(['-x', 'a.scss'])).toBe(
      'Could not find an option named "-x".',
    );
  });
});

describe('splitSourceAndDestination', () => {
  it('respects windows drive letters', () => {
    expect(splitSourceAndDestination('C:\\foo.scss:out.css')).toEqual([
      'C:\\foo.scss',
      'out.css',
    ]);
  });

  it('throws when there is no colon', () => {
    expect(() => splitSourceAndDestination('foo')).toThrowError(
      'Expected "foo" to contain a colon.',
    );
  });
});

describe('relativePath', () => {
  it('computes relative paths with ..', () => {
    expect(relativePath('/a/b', '/a/b')).toBe('.');
    expect(relativePath('/a/b', '/a/b/c.scss')).toBe('c.scss');
    expect(relativePath('/a/b', '/a/c.scss')).toBe('../c.scss');
    expect(relativePath('/a/b', '/d.scss')).toBe('../../d.scss');
  });
});

describe('directory mode', () => {
  it('directory positional is a usage error with a hint', () => {
    const dir = tempDir();
    expect(parseErr([dir, 'out'])).toBe(
      `Directory "${dir}" may not be a positional arg.\nTo compile all CSS in "${dir}" to "out", use \`sass ${dir}:out\`.`,
    );
  });

  it('lists sources under a dir:dest colon arg, skipping partials and css-to-self', () => {
    const dir = tempDir();
    fs.mkdirSync(p.join(dir, 'sub'));
    fs.writeFileSync(p.join(dir, 'a.scss'), 'a { b: c; }');
    fs.writeFileSync(p.join(dir, '_partial.scss'), '');
    fs.writeFileSync(p.join(dir, 'plain.txt'), '');
    fs.writeFileSync(p.join(dir, 'sub', 'b.sass'), '');
    const out = p.join(tempDir(), 'out');
    const o = parse([`${dir}:${out}`]);
    const rel = o.sources.map((s) => [
      p.relative(dir, s.source!),
      p.relative(out, s.dest!),
    ]);
    expect(rel).toEqual([
      ['a.scss', 'a.css'],
      [p.join('sub', 'b.sass'), p.join('sub', 'b.css')],
    ]);
  });

  it('lists all entrypoints for a bare dir colon arg', () => {
    const dir = tempDir();
    fs.writeFileSync(p.join(dir, 'x.scss'), '');
    fs.writeFileSync(p.join(dir, 'y.css'), '');
    const o = parse([`${dir}:${p.join(tempDir(), 'out')}`]);
    expect(o.sources.map((s) => p.basename(s.source!))).toEqual([
      'x.scss',
      'y.css',
    ]);
  });
});
