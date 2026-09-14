import { describe, expect, it } from 'vitest';

import { createBrowserFs, createBrowserFsAsync } from './browser-fs';

// Dart-exact per-method messages (lib/src/io/js.dart: "<method>() is only
// supported on Node.js"). §12 #20: the js-api-spec asserts message substrings,
// so exactness matters for the browser path.
describe('browser fs delegate', () => {
  it('throws Dart-exact UnsupportedError messages per method (sync)', () => {
    const fs = createBrowserFs();
    const expectMessage = (call: () => unknown, method: string): void => {
      try {
        call();
        expect.unreachable('should have thrown');
      } catch (e) {
        expect(e).toMatchObject({
          message: `${method}() is only supported on Node.js`,
          kind: 'Other',
        });
      }
    };
    expectMessage(() => fs.readFile('/a.scss'), 'readFile');
    expectMessage(() => fs.fileExists('/a.scss'), 'fileExists');
    expectMessage(() => fs.dirExists('/a.scss'), 'dirExists');
    expectMessage(() => fs.linkExists('/a.scss'), 'linkExists');
    expectMessage(() => fs.readDir('/'), 'listDir');
  });

  it('rejects with Dart-exact messages per method (async)', async () => {
    const fs = createBrowserFsAsync();
    const expectReject = (
      call: () => Promise<unknown>,
      method: string,
    ): void => {
      expect(call()).rejects.toMatchObject({
        message: `${method}() is only supported on Node.js`,
        kind: 'Other',
      });
    };
    expectReject(() => fs.readFile('/a.scss'), 'readFile');
    expectReject(() => fs.fileExists('/a.scss'), 'fileExists');
    expectReject(() => fs.dirExists('/a.scss'), 'dirExists');
    expectReject(() => fs.linkExists('/a.scss'), 'linkExists');
  });

  it('reports a browser-ish environment', () => {
    const fs = createBrowserFs();
    expect(fs.currentDir()).toBe('/');
    expect(fs.isWindows()).toBe(false);
    expect(fs.isMacOS()).toBe(false);
  });
});
