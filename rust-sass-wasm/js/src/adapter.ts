// Converts between the wasm wire values (plain JS objects — see
// docs/ref/wasm.md, "Value marshalling wire format") and the embedded-host-node value classes.
// Replaces the host's `protofier.ts` (which converts to/from protobuf).
//
// One `Adapter` instance is created per compilation (see index.ts). It owns
// the per-compilation `compileContext` symbol and the per-call argument-list
// tracking — no module-level state.

import { OrderedMap } from 'immutable';

import { Value } from './value/index';
import { SassArgumentList } from './value/argument-list';
import { SassBoolean, sassFalse, sassTrue } from './value/boolean';
import {
  CalculationInterpolation,
  CalculationOperation,
  CalculationValue,
  SassCalculation,
} from './value/calculations';
import { KnownColorSpace, SassColor } from './value/color';
import { SassFunction } from './value/function';
import { ListSeparator, SassList } from './value/list';
import { SassMap } from './value/map';
import { SassMixin } from './value/mixin';
import { SassNumber } from './value/number';
import { sassNull } from './value/null';
import { SassString } from './value/string';
import { deprecations } from './deprecations';

// ==== Wire types (mirror of rust-sass-wasm/src/marshaller.rs) ===============

export type WireValue =
  | { type: 'boolean'; value: boolean }
  | { type: 'null' }
  | { type: 'string'; text: string; quoted: boolean }
  | {
      type: 'number';
      value: number;
      numeratorUnits: string[];
      denominatorUnits: string[];
    }
  | {
      type: 'color';
      space: string;
      channel1?: number;
      channel2?: number;
      channel3?: number;
      alpha?: number;
      missing: boolean[];
    }
  | {
      type: 'list';
      separator: string;
      hasBrackets: boolean;
      contents: WireValue[];
    }
  | {
      type: 'argumentList';
      id: number;
      separator: string;
      hasBrackets: boolean;
      contents: WireValue[];
      keywords: Record<string, WireValue>;
    }
  | { type: 'map'; entries: { key: WireValue; value: WireValue }[] }
  | { type: 'calculation'; name: string; arguments: WireCalcArg[] }
  | { type: 'function'; id: number }
  | { type: 'mixin'; id: number }
  | { type: 'hostFunction'; signature: string; callback: Function };

export type WireCalcArg =
  | {
      type: 'number';
      value: number;
      numeratorUnits: string[];
      denominatorUnits: string[];
    }
  | { type: 'string'; value: string; quoted: boolean }
  | { type: 'interpolation'; value: string }
  | {
      type: 'operation';
      operator: string;
      left: WireCalcArg;
      right: WireCalcArg;
    }
  | { type: 'calculation'; name: string; arguments: WireCalcArg[] };

/** The result of a wrapped custom-function call. */
export interface WireFunctionResult {
  value: WireValue;
  accessedArgumentLists: number[];
}

export type CustomFunction = (args: Value[]) => Value | Promise<Value>;

/** A custom function as wired to Rust (wire args in, wire result out). */
export type WrappedFunction = (
  wasmArgs: WireValue[],
) => WireFunctionResult | Promise<WireFunctionResult>;

/** Contextual information passed to `canonicalize`/`findFileUrl`
 * (js-api-doc `CanonicalizeContext`: `fromImport: boolean; containingUrl:
 * URL | null`). The shim converts the wire string to a `URL` instance. */
export interface CanonicalizeContext {
  fromImport: boolean;
  containingUrl: URL | null;
}

/** A JS importer (URL importer: canonicalize + load, or a file importer). */
export interface JsImporter {
  canonicalize?: (url: string, context: CanonicalizeContext) => unknown;
  load?: (url: URL) => unknown;
  findFileUrl?: (url: string, context: CanonicalizeContext) => unknown;
  nonCanonicalScheme?: string | string[];
}

// ==== pure helpers ===========================================================

function separatorToWasm(sep: ListSeparator): string {
  switch (sep) {
    case ',':
      return 'comma';
    case ' ':
      return 'space';
    case '/':
      return 'slash';
    default:
      return 'undecided';
  }
}

function separatorFromWasm(sep: string): ListSeparator {
  switch (sep) {
    case 'comma':
      return ',';
    case 'space':
      return ' ';
    case 'slash':
      return '/';
    default:
      return null;
  }
}

function operatorToWasm(op: '+' | '-' | '*' | '/'): string {
  switch (op) {
    case '+':
      return 'plus';
    case '-':
      return 'minus';
    case '*':
      return 'times';
    case '/':
      return 'dividedBy';
  }
}

function operatorFromWasm(op: string): '+' | '-' | '*' | '/' {
  switch (op) {
    case 'plus':
      return '+';
    case 'minus':
      return '-';
    case 'times':
      return '*';
    case 'dividedBy':
      return '/';
    default:
      throw new Error(`Unknown CalculationOperator "${op}"`);
  }
}

// ==== the adapter ============================================================

export class Adapter {
  /** The per-compilation context shared by all values created this compile. */
  readonly compileContext: symbol = Symbol('sass-compilation');

  /** Argument lists deprotofied so far in the current call (1-based ids). */
  private argumentLists: SassArgumentList[] = [];

  // ---- wire -> JS value classes -------------------------------------------

  valueFromWasm(obj: WireValue): Value {
    return this.valueFromWasmInner(obj);
  }

  private valueFromWasmInner(obj: WireValue): Value {
    switch (obj.type) {
      case 'boolean':
        return obj.value ? sassTrue : sassFalse;
      case 'null':
        return sassNull;
      case 'string':
        return new SassString(obj.text, { quotes: obj.quoted });
      case 'number':
        return new SassNumber(obj.value, {
          numeratorUnits: obj.numeratorUnits,
          denominatorUnits: obj.denominatorUnits,
        });
      case 'color':
        return colorFromWasm(obj);
      case 'list':
        return new SassList(
          obj.contents.map((c) => this.valueFromWasmInner(c)),
          {
            separator: separatorFromWasm(obj.separator),
            brackets: obj.hasBrackets,
          },
        );
      case 'argumentList': {
        const keywords: Record<string, Value> = {};
        for (const [key, val] of Object.entries(obj.keywords)) {
          keywords[key] = this.valueFromWasmInner(val);
        }
        const list = new SassArgumentList(
          obj.contents.map((c) => this.valueFromWasmInner(c)),
          keywords,
          separatorFromWasm(obj.separator),
          obj.id,
          this.compileContext,
        );
        this.argumentLists.push(list);
        return list;
      }
      case 'map':
        return new SassMap(
          OrderedMap(
            obj.entries.map((entry) => [
              this.valueFromWasmInner(entry.key),
              this.valueFromWasmInner(entry.value),
            ]),
          ),
        );
      case 'calculation':
        return this.calculationFromWasm(obj);
      case 'function':
        return new SassFunction(obj.id, this.compileContext);
      case 'mixin':
        return new SassMixin(obj.id, this.compileContext);
      case 'hostFunction':
        throw new Error('hostFunction values are only valid from JS to Rust');
    }
  }

  private calculationFromWasm(
    obj: Extract<WireValue, { type: 'calculation' }>,
  ): SassCalculation {
    const args = obj.arguments.map((a) => this.calcArgFromWasm(a));
    switch (obj.name) {
      case 'calc':
        return SassCalculation.calc(args[0]);
      case 'clamp':
        return SassCalculation.clamp(args[0], args[1], args[2]);
      case 'min':
        return SassCalculation.min(args);
      case 'max':
        return SassCalculation.max(args);
      default:
        throw new Error(
          `Value.Calculation.name "${obj.name}" is not a recognized calculation type.`,
        );
    }
  }

  private calcArgFromWasm(arg: WireCalcArg): CalculationValue {
    switch (arg.type) {
      case 'number':
        return new SassNumber(arg.value, {
          numeratorUnits: arg.numeratorUnits,
          denominatorUnits: arg.denominatorUnits,
        });
      case 'string':
        return new SassString(arg.value, { quotes: arg.quoted });
      case 'interpolation':
        return new SassString(arg.value, { quotes: false });
      case 'operation':
        return new CalculationOperation(
          operatorFromWasm(arg.operator),
          this.calcArgFromWasm(arg.left),
          this.calcArgFromWasm(arg.right),
        );
      case 'calculation':
        return this.calculationFromWasm(arg);
    }
  }

  // ---- JS value classes -> wire -------------------------------------------

  /**
   * Converts a JS `Value` (custom-function result) to its wire representation,
   * collecting the ids of argument lists whose keywords were accessed.
   */
  valueToWasm(v: Value): WireFunctionResult {
    const value = this.toWire(v);
    const accessedArgumentLists = this.argumentLists
      .filter((list) => list.keywordsAccessed && list.id !== undefined)
      .map((list) => list.id!);
    // Reset after collecting so argument lists deprotofied during the current
    // function call are still tracked when the result is serialized.
    this.argumentLists.length = 0;
    return { value, accessedArgumentLists };
  }

  private toWire(v: Value): WireValue {
    if (v instanceof SassArgumentList) {
      const keywords: Record<string, WireValue> = {};
      for (const [key, val] of v.keywordsInternal) {
        keywords[key] = this.toWire(val);
      }
      return {
        type: 'argumentList',
        // An argument list from another compilation is passed BY VALUE (id 0
        // → Rust builds a fresh list from contents/keywords), matching the
        // embedded host protofier (`compileContext === this.functions.
        // compileContext` → `{id}`, else protofy contents/keywords).
        id: v.compileContext === this.compileContext ? (v.id ?? 0) : 0,
        separator: separatorToWasm(v.separator),
        hasBrackets: v.hasBrackets,
        contents: v.asList.toArray().map((c) => this.toWire(c)),
        keywords,
      };
    }
    if (v instanceof SassBoolean) {
      return { type: 'boolean', value: v.value };
    }
    if (v instanceof SassList) {
      return {
        type: 'list',
        separator: separatorToWasm(v.separator),
        hasBrackets: v.hasBrackets,
        contents: v.asList.toArray().map((c) => this.toWire(c)),
      };
    }
    if (v instanceof SassMap) {
      return {
        type: 'map',
        entries: v.contents.toArray().map(([key, value]) => ({
          key: this.toWire(key),
          value: this.toWire(value),
        })),
      };
    }
    if (v instanceof SassCalculation) {
      return {
        type: 'calculation',
        name: v.name,
        arguments: v.arguments.toArray().map((a) => this.calcArgToWire(a)),
      };
    }
    if (v instanceof SassColor) {
      const channels = v.channelsOrNull;
      return {
        type: 'color',
        space: v.space,
        channel1: channels.get(0) ?? undefined,
        channel2: channels.get(1) ?? undefined,
        channel3: channels.get(2) ?? undefined,
        alpha: v.isChannelMissing('alpha') ? undefined : v.alpha,
        missing: [
          channels.get(0) === null,
          channels.get(1) === null,
          channels.get(2) === null,
          v.isChannelMissing('alpha'),
        ],
      };
    }
    if (v instanceof SassNumber) {
      return {
        type: 'number',
        value: v.value,
        numeratorUnits: v.numeratorUnits.toArray(),
        denominatorUnits: v.denominatorUnits.toArray(),
      };
    }
    if (v instanceof SassString) {
      return { type: 'string', text: v.text, quoted: v.hasQuotes };
    }
    if (v === sassNull) {
      return { type: 'null' };
    }
    if (v instanceof SassFunction) {
      // A compiler function reference from another compilation must not be
      // sent by id (Rust resolves ids in the current compilation's registry) —
      // throw, matching dart-sass `SassFunction.assertCompileContext`.
      if (v.id !== undefined && v.compileContext !== this.compileContext) {
        throw new Error(`${v} does not belong to this compilation`);
      }
      return v.id !== undefined
        ? { type: 'function', id: v.id }
        : {
            type: 'hostFunction',
            signature: v.signature!,
            callback: v.callback!,
          };
    }
    if (v instanceof SassMixin) {
      if (v.compileContext !== this.compileContext) {
        throw new Error(`${v} does not belong to this compilation`);
      }
      return { type: 'mixin', id: v.id };
    }
    throw new Error(`${v} is not a sass.Value.`);
  }

  private calcArgToWire(arg: CalculationValue): WireCalcArg {
    if (arg instanceof SassNumber) {
      return {
        type: 'number',
        value: arg.value,
        numeratorUnits: arg.numeratorUnits.toArray(),
        denominatorUnits: arg.denominatorUnits.toArray(),
      };
    }
    if (arg instanceof SassCalculation) {
      return {
        type: 'calculation',
        name: arg.name,
        arguments: arg.arguments.toArray().map((a) => this.calcArgToWire(a)),
      };
    }
    if (arg instanceof SassString) {
      return { type: 'string', value: arg.text, quoted: arg.hasQuotes };
    }
    if (arg instanceof CalculationOperation) {
      return {
        type: 'operation',
        operator: operatorToWasm(arg.operator),
        left: this.calcArgToWire(arg.left),
        right: this.calcArgToWire(arg.right),
      };
    }
    if (arg instanceof CalculationInterpolation) {
      return { type: 'interpolation', value: arg.value };
    }
    throw new Error(`Invalid calculation value: ${arg}`);
  }

  // ---- host-callback wrapping ----------------------------------------------

  /** Wraps a user custom function for the wasm bridge. */
  wrapFunction(fn: CustomFunction, sync: boolean): WrappedFunction {
    if (sync) {
      return (wasmArgs) => {
        const args = wasmArgs.map((a) => this.valueFromWasmInner(a));
        const result = fn(args);
        if (result instanceof Promise) {
          throw new Error(
            "can't return a Promise for synchronous compile functions",
          );
        }
        return this.valueToWasm(result);
      };
    }
    return async (wasmArgs) => {
      const args = wasmArgs.map((a) => this.valueFromWasmInner(a));
      const result = await fn(args);
      return this.valueToWasm(result);
    };
  }

  /** Wraps a user importer for the wasm bridge (canonicalize + load / findFileUrl).
   *
   * Results are passed through RAW — Rust performs the Dart-exact validation
   * (URL instance checks, sync-Promise detection, exact error messages). The
   * only adaptations are that `load` receives a `URL` instance per the
   * js-api-doc (`load(canonicalUrl: URL)`), not a plain string, and that the
   * `CanonicalizeContext` handed to `canonicalize`/`findFileUrl` exposes
   * `containingUrl` as a `URL` instance (js-api-doc: `containingUrl: URL |
   * null`), not the wire string. */
  wrapImporter(importer: JsImporter, sync: boolean): Record<string, unknown> {
    const context = (ctx: {
      fromImport: boolean;
      containingUrl?: string;
    }): {
      fromImport: boolean;
      containingUrl: URL | null;
    } => ({
      fromImport: ctx.fromImport,
      containingUrl: ctx.containingUrl ? new URL(ctx.containingUrl) : null,
    });
    const out: Record<string, unknown> = {};
    if (importer.canonicalize) {
      const fn = importer.canonicalize;
      out.canonicalize = sync
        ? (url: string, ctx: { fromImport: boolean; containingUrl?: string }) =>
            fn(url, context(ctx))
        : async (
            url: string,
            ctx: { fromImport: boolean; containingUrl?: string },
          ) => await fn(url, context(ctx));
    }
    if (importer.load) {
      const fn = importer.load;
      out.load = sync
        ? (url: string) => fn(new URL(url))
        : async (url: string) => await fn(new URL(url));
    }
    if (importer.findFileUrl) {
      const fn = importer.findFileUrl;
      out.findFileUrl = sync
        ? (url: string, ctx: { fromImport: boolean; containingUrl?: string }) =>
            fn(url, context(ctx))
        : async (
            url: string,
            ctx: { fromImport: boolean; containingUrl?: string },
          ) => await fn(url, context(ctx));
    }
    if (importer.nonCanonicalScheme !== undefined) {
      out.nonCanonicalScheme = importer.nonCanonicalScheme;
    }
    return out;
  }

  /**
   * Wraps a user logger for the wasm bridge. Maps the Rust-sent
   * `deprecationType` **id string** back to the full `deprecations` map entry
   * (js-api-spec asserts `warn`'s `deprecationType` `toEqual(deprecations[id])`).
   * Returns `undefined` when the logger provides neither `warn` nor `debug` so
   * the option is dropped entirely.
   */
  wrapLogger(
    logger: { warn?: Function; debug?: Function },
    sync: boolean,
  ): Record<string, unknown> | undefined {
    const out: Record<string, unknown> = {};
    if (typeof logger.warn === 'function') {
      const warn = logger.warn;
      const wrapped = (
        message: string,
        options: Record<string, unknown>,
      ): unknown => warn(message, this.mapLoggerOptions(options));
      out.warn = sync
        ? wrapped
        : async (message: string, options: Record<string, unknown>) =>
            await wrapped(message, options);
    }
    if (typeof logger.debug === 'function') {
      const debug = logger.debug;
      out.debug = sync
        ? (message: string, options: Record<string, unknown>) =>
            debug(message, this.mapLoggerOptions(options))
        : async (message: string, options: Record<string, unknown>) =>
            await debug(message, this.mapLoggerOptions(options));
    }
    return out.warn === undefined && out.debug === undefined ? undefined : out;
  }

  /**
   * Adapts the Rust-sent `WarnOptions`/`DebugOptions` wire shape to the public
   * JS API: `deprecationType` id string → the `deprecations` map entry, and
   * `span.url` string → a `URL` instance (js-api-doc: `url?: URL`). Returns the
   * original object untouched when nothing needs adapting.
   */
  private mapLoggerOptions(
    options: Record<string, unknown>,
  ): Record<string, unknown> {
    const { deprecationType, span, ...rest } = options;
    if (typeof deprecationType !== 'string' && !span) return options;
    const out: Record<string, unknown> = { ...rest };
    if (typeof deprecationType === 'string') {
      out.deprecationType = deprecations[deprecationType];
    }
    if (span) {
      out.span = {
        ...(span as Record<string, unknown>),
        url: (span as { url?: string | null }).url
          ? new URL((span as { url: string }).url)
          : undefined,
      };
    }
    return out;
  }
}

function colorFromWasm(obj: Extract<WireValue, { type: 'color' }>): SassColor {
  const channel1 = obj.channel1 ?? null;
  const channel2 = obj.channel2 ?? null;
  const channel3 = obj.channel3 ?? null;
  const alpha = obj.alpha ?? null;
  const space = obj.space as KnownColorSpace;
  switch (space.toLowerCase()) {
    case 'rgb':
    case 'srgb':
    case 'srgb-linear':
    case 'display-p3':
    case 'display-p3-linear':
    case 'a98-rgb':
    case 'prophoto-rgb':
    case 'rec2020':
      return new SassColor({
        red: channel1,
        green: channel2,
        blue: channel3,
        alpha,
        space,
      });
    case 'hsl':
      return new SassColor({
        hue: channel1,
        saturation: channel2,
        lightness: channel3,
        alpha,
        space,
      });
    case 'hwb':
      return new SassColor({
        hue: channel1,
        whiteness: channel2,
        blackness: channel3,
        alpha,
        space,
      });
    case 'lab':
    case 'oklab':
      return new SassColor({
        lightness: channel1,
        a: channel2,
        b: channel3,
        alpha,
        space,
      });
    case 'lch':
    case 'oklch':
      return new SassColor({
        lightness: channel1,
        chroma: channel2,
        hue: channel3,
        alpha,
        space,
      });
    case 'xyz':
    case 'xyz-d65':
    case 'xyz-d50':
      return new SassColor({
        x: channel1,
        y: channel2,
        z: channel3,
        alpha,
        space,
      });
    default:
      throw new Error(`Unknown color space "${space}"`);
  }
}
