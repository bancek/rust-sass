import { afterAll, describe, expect, it } from 'vitest';
import * as sass from 'sass';
import * as fs from 'node:fs';
import * as p from 'node:path';

import { NodePackageImporter, compileString } from '../index';
import { artifactsPresent, tempDir } from './helpers';

// Builds a temp tree with a `node_modules/<pkg>` package and returns the tree
// root. `sassField` controls the package.json `sass` field (or none).
function makePackageTree(): { dir: string; pkgDir: string } {
  const dir = tempDir();
  const pkgDir = p.join(dir, 'node_modules', 'mytheme');
  fs.mkdirSync(pkgDir, { recursive: true });
  return { dir, pkgDir };
}

describe.skipIf(!artifactsPresent)(
  'NodePackageImporter parity with sass',
  () => {
    const dirs: string[] = [];
    afterAll(() => {
      for (const dir of dirs) fs.rmSync(dir, { recursive: true, force: true });
    });

    it('resolves pkg: URLs through the package.json sass field', () => {
      const { dir, pkgDir } = makePackageTree();
      dirs.push(dir);
      fs.writeFileSync(
        p.join(pkgDir, 'package.json'),
        JSON.stringify({ name: 'mytheme', sass: 'index.scss' }),
      );
      fs.writeFileSync(
        p.join(pkgDir, 'index.scss'),
        '$c: red; .t { color: $c; }',
      );

      const source = '@use "pkg:mytheme";\n.x { color: mytheme.$c; }';
      const ours = compileString(source, {
        importers: [new NodePackageImporter(dir)],
      });
      const theirs = sass.compileString(source, {
        importers: [new sass.NodePackageImporter(dir)],
      });
      expect(ours.css).toBe(theirs.css);
      // Exact loadedUrls parity: our canonicalize is lexical + case-correcting
      // (preserving symlink dir names like `/var`), matching Dart (docs/ref/wasm.md, "Io bridge").
      expect(ours.loadedUrls.map(String)).toEqual(
        theirs.loadedUrls.map(String),
      );
    });

    it('resolves a bare pkg: URL to _index.scss at the package root', () => {
      const { dir, pkgDir } = makePackageTree();
      dirs.push(dir);
      fs.writeFileSync(
        p.join(pkgDir, 'package.json'),
        JSON.stringify({ name: 'mytheme' }),
      );
      fs.writeFileSync(p.join(pkgDir, '_index.scss'), '$c: red;');

      const source = '@use "pkg:mytheme";\n.x { color: mytheme.$c; }';
      expect(
        compileString(source, { importers: [new NodePackageImporter(dir)] })
          .css,
      ).toBe(
        sass.compileString(source, {
          importers: [new sass.NodePackageImporter(dir)],
        }).css,
      );
    });

    it('resolves pkg: subpaths', () => {
      const { dir, pkgDir } = makePackageTree();
      dirs.push(dir);
      fs.mkdirSync(p.join(pkgDir, 'sub'), { recursive: true });
      fs.writeFileSync(
        p.join(pkgDir, 'package.json'),
        JSON.stringify({ name: 'mytheme' }),
      );
      fs.writeFileSync(p.join(pkgDir, '_index.scss'), '$root: red;');
      fs.writeFileSync(p.join(pkgDir, 'sub', '_index.scss'), '$sub: blue;');

      const source = '@use "pkg:mytheme/sub";\n.x { color: sub.$sub; }';
      expect(
        compileString(source, { importers: [new NodePackageImporter(dir)] })
          .css,
      ).toBe(
        sass.compileString(source, {
          importers: [new sass.NodePackageImporter(dir)],
        }).css,
      );
    });
  },
);
