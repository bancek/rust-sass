// The full `deprecations` map, generated from
// `dart-sass/sass/spec/deprecations.yaml` (single source of truth — the
// colocated `deprecations.test.ts` guards drift) plus the special
// `user-authored` entry (status 'user'), matching the official `sass` package.
// `calc-interp` is deliberately excluded (lib/src/js/deprecations.dart:32-34).
//
// dart-source: lib/src/js/deprecations.dart + sass/js-api-doc/deprecations.d.ts
// (the host-side handling is ported from embedded-host-node/lib/src/deprecations.ts)

import { Version } from './version';
import type { CompileOptions } from './options';

export interface Deprecation {
  /** The unique ID of this deprecation. */
  id: string;

  /** The current status of this deprecation. */
  status: 'future' | 'user' | 'active' | 'obsolete';

  /** A human-readable description of this deprecation. */
  description: string | null;

  /** The version this deprecation first became active in. */
  deprecatedIn: Version | null;

  /** The version this deprecation became obsolete in. */
  obsoleteIn: Version | null;
}

// [id, status, description, deprecatedIn?, obsoleteIn?] from deprecations.yaml.
const entries: Array<{
  id: string;
  status: 'active' | 'obsolete' | 'user';
  description: string | null;
  deprecatedIn?: string;
  obsoleteIn?: string;
}> = [
  {
    id: 'call-string',
    status: 'active',
    description: 'Passing a string directly to meta.call().',
    deprecatedIn: '0.0.0',
  },
  {
    id: 'elseif',
    status: 'active',
    description: '@elseif.',
    deprecatedIn: '1.3.2',
  },
  {
    id: 'moz-document',
    status: 'active',
    description: '@-moz-document.',
    deprecatedIn: '1.7.2',
  },
  {
    id: 'relative-canonical',
    status: 'active',
    description: 'Imports using relative canonical URLs.',
    deprecatedIn: '1.14.2',
  },
  {
    id: 'new-global',
    status: 'active',
    description: 'Declaring new variables with !global.',
    deprecatedIn: '1.17.2',
  },
  {
    id: 'color-module-compat',
    status: 'active',
    description:
      'Using color module functions in place of plain CSS functions.',
    deprecatedIn: '1.23.0',
  },
  {
    id: 'slash-div',
    status: 'active',
    description: '/ operator for division.',
    deprecatedIn: '1.33.0',
  },
  {
    id: 'bogus-combinators',
    status: 'active',
    description: 'Leading, trailing, and repeated combinators.',
    deprecatedIn: '1.54.0',
  },
  {
    id: 'strict-unary',
    status: 'active',
    description: 'Ambiguous + and - operators.',
    deprecatedIn: '1.55.0',
  },
  {
    id: 'function-units',
    status: 'active',
    description: 'Passing invalid units to built-in functions.',
    deprecatedIn: '1.56.0',
  },
  {
    id: 'duplicate-var-flags',
    status: 'active',
    description: 'Using !default or !global multiple times for one variable.',
    deprecatedIn: '1.62.0',
  },
  {
    id: 'null-alpha',
    status: 'active',
    description: 'Passing null as alpha in the $PLATFORM API.',
    deprecatedIn: '1.62.3',
  },
  {
    id: 'abs-percent',
    status: 'active',
    description: 'Passing percentages to the Sass abs() function.',
    deprecatedIn: '1.65.0',
  },
  {
    id: 'fs-importer-cwd',
    status: 'active',
    description:
      'Using the current working directory as an implicit load path.',
    deprecatedIn: '1.73.0',
  },
  {
    id: 'css-function-mixin',
    status: 'obsolete',
    description: 'Function and mixin names beginning with --.',
    deprecatedIn: '1.76.0',
    obsoleteIn: '1.94.0',
  },
  {
    id: 'mixed-decls',
    status: 'obsolete',
    description: 'Declarations after or between nested rules.',
    deprecatedIn: '1.77.7',
    obsoleteIn: '1.92.0',
  },
  {
    id: 'feature-exists',
    status: 'active',
    description: 'meta.feature-exists',
    deprecatedIn: '1.78.0',
  },
  {
    id: 'color-4-api',
    status: 'active',
    description: 'Certain uses of built-in sass:color functions.',
    deprecatedIn: '1.79.0',
  },
  {
    id: 'color-functions',
    status: 'active',
    description: 'Using global color functions instead of sass:color.',
    deprecatedIn: '1.79.0',
  },
  {
    id: 'legacy-js-api',
    status: 'active',
    description: 'Legacy JS API.',
    deprecatedIn: '1.79.0',
  },
  {
    id: 'import',
    status: 'active',
    description: '@import rules.',
    deprecatedIn: '1.80.0',
  },
  {
    id: 'global-builtin',
    status: 'active',
    description:
      'Global built-in functions that are available in sass: modules.',
    deprecatedIn: '1.80.0',
  },
  {
    id: 'type-function',
    status: 'obsolete',
    description: 'Functions named "type".',
    deprecatedIn: '1.86.0',
    obsoleteIn: '1.92.0',
  },
  {
    id: 'compile-string-relative-url',
    status: 'active',
    description: 'Passing a relative url to compileString().',
    deprecatedIn: '1.88.0',
  },
  {
    id: 'misplaced-rest',
    status: 'active',
    description: 'A rest parameter before a positional or named parameter.',
    deprecatedIn: '1.91.0',
  },
  {
    id: 'with-private',
    status: 'active',
    description:
      'Configuring private variables in @use, @forward, or load-css().',
    deprecatedIn: '1.92.0',
  },
  {
    id: 'if-function',
    status: 'active',
    description: 'The Sass if($condition, $if-true, $if-false) function.',
    deprecatedIn: '1.95.0',
  },
  {
    id: 'function-name',
    status: 'active',
    description: 'Uppercase reserved function names.',
    deprecatedIn: '1.98.0',
  },
  {
    id: 'adjacent-compounds',
    status: 'active',
    description: 'Adjacent compound selectors like `[class]a`.',
    deprecatedIn: '1.100.0',
  },
  {
    id: 'user-authored',
    status: 'user',
    description: null,
  },
];

export const deprecations: Record<string, Deprecation> = Object.fromEntries(
  entries.map((entry) => [
    entry.id,
    {
      id: entry.id,
      status: entry.status,
      description: entry.description,
      deprecatedIn: entry.deprecatedIn
        ? Version.parse(entry.deprecatedIn)
        : null,
      obsoleteIn: entry.obsoleteIn ? Version.parse(entry.obsoleteIn) : null,
    },
  ]),
);

// ==== host-side deprecation handling ==========================================

export type DeprecationOrId = Deprecation | string;

/** The subset of compile options relevant to deprecation handling. */
export type DeprecationOptions = Pick<
  CompileOptions,
  'fatalDeprecations' | 'futureDeprecations' | 'silenceDeprecations'
>;

/**
 * Map between active compilations and the deprecation options they use. Host
 * side deprecation warnings (e.g. legacy color channel getters on a JS value)
 * consult this when not tied to a specific compilation (embedded-host-node
 * `activeDeprecationOptions`).
 */
export const activeDeprecationOptions: Map<symbol, DeprecationOptions> =
  new Map();

/** Converts a mixed array of deprecations/ids/versions to ids. */
function getDeprecationIds(arr: (DeprecationOrId | Version)[]): string[] {
  return arr.map((item) => {
    if (item instanceof Version) return item.toString();
    if (typeof item === 'string') return item;
    return item.id;
  });
}

/**
 * Handles a host-side deprecation warning, either emitting a warning, throwing
 * an error, or doing nothing depending on the deprecation options used.
 *
 * If no specific deprecation options are passed here, options are determined
 * based on the options of the active compilations.
 */
export function warnForHostSideDeprecation(
  message: string,
  deprecation: Deprecation,
  options?: DeprecationOptions,
): void {
  if (
    deprecation.status === 'future' &&
    !isEnabledFuture(deprecation, options)
  ) {
    return;
  }
  const fullMessage = `Deprecation [${deprecation.id}]: ${message}`;
  if (isFatal(deprecation, options)) {
    throw Error(fullMessage);
  }
  if (!isSilent(deprecation, options)) {
    console.warn(fullMessage);
  }
}

/**
 * Checks whether the given deprecation is in the given list of silent
 * deprecations or is silenced by at least one active compilation.
 */
function isSilent(
  deprecation: Deprecation,
  options?: DeprecationOptions,
): boolean {
  if (!options) {
    for (const potentialOptions of activeDeprecationOptions.values()) {
      if (isSilent(deprecation, potentialOptions)) return true;
    }
    return false;
  }
  return getDeprecationIds(options.silenceDeprecations ?? []).includes(
    deprecation.id,
  );
}

/**
 * Checks whether the given deprecation is in the given list of future
 * deprecations that should be enabled or is enabled in all active compilations.
 */
function isEnabledFuture(
  deprecation: Deprecation,
  options?: DeprecationOptions,
): boolean {
  if (!options) {
    for (const potentialOptions of activeDeprecationOptions.values()) {
      if (!isEnabledFuture(deprecation, potentialOptions)) return false;
    }
    return activeDeprecationOptions.size > 0;
  }
  return getDeprecationIds(options.futureDeprecations ?? []).includes(
    deprecation.id,
  );
}

/**
 * Checks whether the given deprecation is in the given list of fatal
 * deprecations or is marked as fatal in all active compilations.
 */
function isFatal(
  deprecation: Deprecation,
  options?: DeprecationOptions,
): boolean {
  if (!options) {
    for (const potentialOptions of activeDeprecationOptions.values()) {
      if (!isFatal(deprecation, potentialOptions)) return false;
    }
    return activeDeprecationOptions.size > 0;
  }
  const versionNumber =
    deprecation.deprecatedIn === null
      ? null
      : deprecation.deprecatedIn.major * 1000000 +
        deprecation.deprecatedIn.minor * 1000 +
        deprecation.deprecatedIn.patch;
  for (const fatal of options.fatalDeprecations ?? []) {
    if (fatal instanceof Version) {
      if (versionNumber === null) continue;
      if (deprecation.obsoleteIn !== null) continue;
      if (
        versionNumber <=
        fatal.major * 1000000 + fatal.minor * 1000 + fatal.patch
      ) {
        return true;
      }
    } else if (typeof fatal === 'string') {
      if (fatal === deprecation.id) return true;
    } else {
      if (fatal.id === deprecation.id) return true;
    }
  }
  return false;
}
