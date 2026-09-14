package main

import (
	"bytes"
	"fmt"
	"log"
	"os"

	libsass "github.com/wellington/go-libsass"
)

func main() {
	var buf bytes.Buffer
	in := bytes.NewBufferString("$color: red; .foo { color: $color; }")
	comp, err := libsass.New(&buf, in)
	if err != nil {
		log.Fatal(err)
	}
	if err := comp.Run(); err != nil {
		log.Fatal(err)
	}
	want := ".foo {\n  color: red; }\n"
	if buf.String() != want {
		log.Fatalf("got %q want %q", buf.String(), want)
	}
	fmt.Fprintln(os.Stderr, "wellington SMOKE-OK")
}
