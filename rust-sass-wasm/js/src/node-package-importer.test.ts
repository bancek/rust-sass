import { afterEach, describe, expect, it } from 'vitest';
import * as p from 'node:path';

import {
  NodePackageImporter,
  _setEntrypointFilename,
} from './node-package-importer';

function entryPointOf(importer: NodePackageImporter): string {
  return (importer as unknown as { entryPointDirectory: string })
    .entryPointDirectory;
}

describe('NodePackageImporter', () => {
  afterEach(() => _setEntrypointFilename(undefined));

  it('uses the provided entry point directory', () => {
    const importer = new NodePackageImporter('/some/dir');
    expect(entryPointOf(importer)).toBe(p.resolve('/some/dir'));
  });

  it('derives the entry point directory from the entrypoint filename', () => {
    _setEntrypointFilename(() => p.join(p.resolve('/app'), 'main.js'));
    const importer = new NodePackageImporter();
    expect(entryPointOf(importer)).toBe(p.resolve('/app'));
  });

  it('throws when no entry point can be determined', () => {
    _setEntrypointFilename(() => null);
    expect(() => new NodePackageImporter()).toThrowError(
      /cannot determine an entry point because `require\.main\.filename` is not defined\. Please provide an `entryPointDirectory` to the `NodePackageImporter`\./,
    );
  });
});
