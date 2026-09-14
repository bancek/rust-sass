// Copyright 2024 Google LLC. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Copied from embedded-host-node/lib/src/version.ts; the `./vendor/sass` type
// import is replaced with a local interface so this module is standalone.

interface SassVersionApi {
  readonly major: number;
  readonly minor: number;
  readonly patch: number;
}

export class Version implements SassVersionApi {
  constructor(
    readonly major: number,
    readonly minor: number,
    readonly patch: number,
  ) {}
  static parse(version: string): Version {
    const match = version.match(/^(\d+)\.(\d+)\.(\d+)$/);
    if (match === null) {
      throw new Error(`Invalid version ${version}`);
    }
    return new Version(
      parseInt(match[1]),
      parseInt(match[2]),
      parseInt(match[3]),
    );
  }
  toString(): string {
    return `${this.major}.${this.minor}.${this.patch}`;
  }
}

/** The Sass version reported by `--version` and used as the cap for
 * `--fatal-deprecation` version arguments. Matches the `info` version and the
 * tracked dart-sass port version (see `docs/upstream.md`). */
export const SASS_VERSION = '1.104.0';

/** The Dart compiler version in the `--version` line and `info` string. */
export const DART2JS_VERSION = '3.13.3';

/** The `--version` line. Matches `sass.js`. */
export const VERSION_TEXT = `${SASS_VERSION} compiled with dart2js ${DART2JS_VERSION}`;

/** Version and implementation metadata for the `info` export. */
export const INFO_TEXT =
  `dart-sass\t${SASS_VERSION}\t(Sass Compiler)\t[Dart]\n` +
  `dart2js\t${DART2JS_VERSION}\t(Dart Compiler)\t[Dart]`;
