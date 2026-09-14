import { describe, expect, it } from 'vitest';

import { Logger } from './logger';

describe('Logger', () => {
  it('exposes a silent logger', () => {
    expect(Logger.silent.warn).toBeTypeOf('function');
    expect(Logger.silent.debug).toBeTypeOf('function');
  });

  it('silent warn/debug are no-ops', () => {
    expect(() => Logger.silent.warn('message', {} as never)).not.toThrow();
    expect(() => Logger.silent.debug('message', {} as never)).not.toThrow();
  });
});
