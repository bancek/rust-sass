import sass

assert sass.libsass_version == "3.6.6", sass.libsass_version
out = sass.compile(string="$color: red; .foo { color: $color; }")
assert out == ".foo {\n  color: red; }\n", repr(out)
print("python SMOKE-OK")
