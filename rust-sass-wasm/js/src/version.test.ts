import { describe, expect, it } from 'vitest';

import { Version } from './version';

describe('Version', () => {
  it('exposes major/minor/patch', () => {
    expect(new Version(1, 102, 0)).toEqual({ major: 1, minor: 102, patch: 0 });
  });

  it('parses a version string', () => {
    expect(Version.parse('1.2.3')).toEqual(new Version(1, 2, 3));
    expect(Version.parse('1.2.3').toString()).toBe('1.2.3');
  });

  it('parses zero-padded segments', () => {
    expect(Version.parse('01.02.003').toString()).toBe('1.2.3');
  });

  it('rejects invalid version strings', () => {
    expect(() => Version.parse('1.2')).toThrowError('Invalid version 1.2');
    expect(() => Version.parse('a.b.c')).toThrowError('Invalid version a.b.c');
    expect(() => Version.parse('1.2.3.4')).toThrowError(
      'Invalid version 1.2.3.4',
    );
  });
});
