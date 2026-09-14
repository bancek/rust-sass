use strict;
use warnings;
use CSS::Sass;

my $sass = CSS::Sass->new;
die "version mismatch" unless CSS::Sass::libsass_version() eq "3.6.6";
my $css = $sass->compile('$color: red; .foo { color: $color; }');
die "output mismatch: [$css]" unless $css eq ".foo {\n  color: red; }\n";
print "perl SMOKE-OK\n";
