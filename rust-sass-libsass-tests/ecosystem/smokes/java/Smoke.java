import io.bit3.jsass.Compiler;
import io.bit3.jsass.Options;
import io.bit3.jsass.Output;

public class Smoke {
  public static void main(String[] args) throws Exception {
    String version = Compiler.getLibsassVersion();
    if (!"3.6.6".equals(version)) {
      throw new IllegalStateException("libsass version: " + version);
    }
    Compiler compiler = new Compiler();
    Output out = compiler.compileString("$color: red; .foo { color: $color; }", new Options());
    String css = out.getCss();
    if (!css.contains(".foo") || !css.contains("color: red") || css.contains("$color")) {
      throw new IllegalStateException("got: " + css);
    }
    System.out.println("java SMOKE-OK");
  }
}
