// ESM entry point: re-exports the tsc-generated CommonJS build. Node's
// cjs-module-lexer statically detects the named exports of `index.js`, so
// `import { compileString } from 'sass-wasm'` works alongside `require()`.
export * from './index.js';
