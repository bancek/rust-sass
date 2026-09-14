import * as fs from 'node:fs';
import * as os from 'node:os';
import * as p from 'node:path';

import { afterAll, describe, expect, it } from 'vitest';

import { parseArgs } from './args';
import { writeSourceMap } from './compile';
import type { WasmCompileResult } from '../types';

const tempDirs: string[] = [];
function tempDir(): string {
  const dir = fs.mkdtempSync(p.join(os.tmpdir(), 'sass-cli-sm-'));
  tempDirs.push(dir);
  return dir;
}

afterAll(() => {
  for (const dir of tempDirs) fs.rmSync(dir, { recursive: true, force: true });
});

function result(sourceMap?: unknown): WasmCompileResult {
  return { css: '', loadedUrls: [], sourceMap };
}

describe('writeSourceMap', () => {
  it('returns empty when there is no source map', () => {
    const opts = parseArgs(['in.scss', 'out.css'], false);
    expect(writeSourceMap(opts, result(), 'out.css')).toBe('');
  });

  it('remaps file: sources relative to the dest dir and sets file', () => {
    const dir = tempDir();
    const src = p.join(dir, 'sub', 'in.scss');
    const dest = p.join(dir, 'out.css');
    const opts = parseArgs(['--source-map', src, dest], false);
    const out = writeSourceMap(
      opts,
      result({
        version: 3,
        sources: [`file://${src}`],
        names: [],
        mappings: 'AAAA',
      }),
      dest,
    );
    // The comment points at the `.map` file, relative to the dest directory.
    expect(out).toBe('\n\n/*# sourceMappingURL=out.css.map */');
    const mapPath = `${dest}.map`;
    const map = JSON.parse(fs.readFileSync(mapPath, 'utf8'));
    expect(map.file).toBe('out.css');
    expect(map.sources[0]).toBe('sub/in.scss');
  });

  it('leaves data: sources untouched', () => {
    const dir = tempDir();
    const dest = p.join(dir, 'out.css');
    const opts = parseArgs(['in.scss', dest], false);
    const out = writeSourceMap(
      opts,
      result({
        version: 3,
        sources: ['data:;charset=utf-8,abc'],
        names: [],
        mappings: '',
      }),
      dest,
    );
    expect(out).toContain('/*# sourceMappingURL=');
    const map = JSON.parse(fs.readFileSync(`${dest}.map`, 'utf8'));
    expect(map.sources[0]).toBe('data:;charset=utf-8,abc');
  });

  it('embeds a data URL without writing a map file', () => {
    const dir = tempDir();
    const src = p.join(dir, 'in.scss');
    const dest = p.join(dir, 'out.css');
    const opts = parseArgs(
      ['--source-map', '--embed-source-map', src, dest],
      false,
    );
    const out = writeSourceMap(
      opts,
      result({
        version: 3,
        sources: [`file://${src}`],
        names: [],
        mappings: '',
      }),
      dest,
    );
    expect(out).toContain(
      '/*# sourceMappingURL=data:application/json;charset=utf-8,',
    );
    expect(fs.existsSync(`${dest}.map`)).toBe(false);
  });

  it('uses no leading newlines for compressed output', () => {
    const dir = tempDir();
    const dest = p.join(dir, 'out.css');
    const opts = parseArgs(['--style', 'compressed', 'in.scss', dest], false);
    const out = writeSourceMap(
      opts,
      result({ version: 3, sources: [], names: [], mappings: '' }),
      dest,
    );
    expect(out.startsWith('\n\n')).toBe(false);
  });

  it('escapes */ in the sourceMappingURL', () => {
    const dir = tempDir();
    const dest = p.join(dir, 'out.css');
    const opts = parseArgs(
      ['--source-map', '--embed-source-map', 'in.scss', dest],
      false,
    );
    // A data: source whose content contains `*/` lands in the embedded map JSON.
    const out = writeSourceMap(
      opts,
      result({
        version: 3,
        sources: ['data:;charset=utf-8,/* hi */'],
        names: [],
        mappings: '',
      }),
      dest,
    );
    expect(out).toContain('%2A/');
    expect(out).not.toContain(
      'data:application/json;charset=utf-8,{"version":3,"sources":["data:;charset=utf-8,/* hi */"]',
    );
  });
});
