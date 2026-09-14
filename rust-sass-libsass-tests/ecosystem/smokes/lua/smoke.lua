local sass = require "sass"
assert(sass.LIBSASS_VERSION == "3.6.6", sass.LIBSASS_VERSION)
local out = assert(sass.compile("$color: red; .foo { color: $color; }"))
assert(out == ".foo {\n  color: red; }\n", string.format("%q", out))
print("lua SMOKE-OK")
