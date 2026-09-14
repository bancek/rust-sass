# frozen_string_literal: true

require "sassc"

raise SassC::Native.version.inspect unless SassC::Native.version == "3.6.6"
out = SassC::Engine.new('$color: red; .foo { color: $color; }').render
raise out.inspect unless out == ".foo {\n  color: red; }\n"
puts "ruby SMOKE-OK"
