<?php
$sass = new Sass();
$version = $sass->getLibraryVersion();
if ($version !== '3.6.6') { fwrite(STDERR, "version: $version\n"); exit(1); }
$css = $sass->compile('$color: red; .foo { color: $color; }');
if ($css !== ".foo {\n  color: red; }\n") { fwrite(STDERR, "got: [$css]\n"); exit(1); }
echo "php SMOKE-OK\n";
