// Copyright 2020 Google Inc. Use of this source code is governed by an
// MIT-style license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
//
// Local stand-in for `embedded-host-node/lib/src/utils.ts` — the only helpers
// the value classes import.

import { List } from 'immutable';

export function asImmutableList<T>(collection: T[] | List<T>): List<T> {
  return List.isList(collection) ? collection : List(collection);
}

export function valueError(message: string, name?: string): Error {
  return Error(name ? `$${name}: ${message}.` : `${message}.`);
}
