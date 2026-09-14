using System;
using LibSass.Compiler;
using LibSass.Compiler.Options;

public static class Program
{
    public static int Main()
    {
        if (SassCompiler.SassInfo.LibSassVersion != "3.6.6")
        {
            Console.Error.WriteLine("libsass version: " + SassCompiler.SassInfo.LibSassVersion);
            return 1;
        }
        if (SassCompiler.SassInfo.SassLanguageVersion != "3.5")
        {
            Console.Error.WriteLine("language version: " + SassCompiler.SassInfo.SassLanguageVersion);
            return 1;
        }
        var compiler = new SassCompiler(new SassOptions { Data = "$color: red; .foo { color: $color; }" });
        var result = compiler.Compile();
        if (result.Output != ".foo {\n  color: red; }\n")
        {
            Console.Error.WriteLine("got: " + result.Output);
            return 1;
        }
        Console.WriteLine("dotnet SMOKE-OK");
        return 0;
    }
}
