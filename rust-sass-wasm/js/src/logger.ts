// Copyright 2024 Google LLC. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Copied from embedded-host-node/lib/src/logger.ts.

/** An object that can be passed to {@link Options} to control how Sass
 * handles warnings and/or debug messages. */
export const Logger = {
  /** A {@link Logger} that silently ignores all warnings and debug messages. */
  silent: {
    warn() {},
    debug() {},
  },
};
