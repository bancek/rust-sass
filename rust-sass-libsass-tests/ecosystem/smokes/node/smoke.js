"use strict";
const sass = require("/tmp/node-sass/lib");
const lines = sass.info.split("\n");
if (!lines[1].includes("3.6.6")) {
  throw new Error("libsass version: " + lines[1]);
}
const out = sass.renderSync({ data: "$color: red; .foo { color: $color; }" }).css.toString();
if (out !== ".foo {\n  color: red; }\n") {
  throw new Error("got: " + JSON.stringify(out));
}
console.log("node SMOKE-OK");
