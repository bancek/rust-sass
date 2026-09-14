import { describe, expect, it } from 'vitest';

import { toIoError } from './error';

describe('toIoError', () => {
  it('maps ENOENT to NotFound preserving message and path', () => {
    const err = Object.assign(new Error('no such file or directory'), {
      code: 'ENOENT',
      path: '/a.scss',
    });
    expect(toIoError(err)).toEqual({
      message: 'no such file or directory',
      kind: 'NotFound',
      path: '/a.scss',
    });
  });

  it('maps EACCES and EPERM to Permission', () => {
    const access = Object.assign(new Error('permission denied'), {
      code: 'EACCES',
    });
    expect(toIoError(access).kind).toBe('Permission');
    const perm = Object.assign(new Error('operation not permitted'), {
      code: 'EPERM',
    });
    expect(toIoError(perm).kind).toBe('Permission');
  });

  it('maps EEXIST to AlreadyExists', () => {
    const err = Object.assign(new Error('file exists'), { code: 'EEXIST' });
    expect(toIoError(err).kind).toBe('AlreadyExists');
  });

  it('maps unknown codes and non-errors to Other', () => {
    expect(
      toIoError(Object.assign(new Error('boom'), { code: 'EOTHER' })).kind,
    ).toBe('Other');
    expect(toIoError('boom').kind).toBe('Other');
    expect(toIoError('boom').message).toBe('boom');
  });

  it('falls back to the supplied path when the error has none', () => {
    const err = Object.assign(new Error('permission denied'), {
      code: 'EACCES',
    });
    expect(toIoError(err, '/fallback.scss')).toEqual({
      message: 'permission denied',
      kind: 'Permission',
      path: '/fallback.scss',
    });
  });

  it('omits the path when neither the error nor the arg has one', () => {
    expect(
      toIoError(Object.assign(new Error('x'), { code: 'ENOENT' })),
    ).toEqual({
      message: 'x',
      kind: 'NotFound',
    });
  });
});
