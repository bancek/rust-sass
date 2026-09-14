import sass
import sass/libsass

doAssert $version() == "3.6.6", $version()
doAssert $language_version() == "3.5", $language_version()
let css = compile("$color: red; .foo { color: $color; }")
doAssert css == ".foo {\n  color: red; }\n", repr(css)
echo "nim SMOKE-OK"
