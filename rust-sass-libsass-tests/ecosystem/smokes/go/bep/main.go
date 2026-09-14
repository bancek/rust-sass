package main

import (
	"fmt"
	"log"

	"github.com/bep/golibsass/libsass"
)

func main() {
	t, err := libsass.New(libsass.Options{})
	if err != nil {
		log.Fatal(err)
	}
	res, err := t.Execute("$color: red; .foo { color: $color; }")
	if err != nil {
		log.Fatal(err)
	}
	want := ".foo {\n  color: red; }\n"
	if res.CSS != want {
		log.Fatalf("got %q want %q", res.CSS, want)
	}
	fmt.Println("bep SMOKE-OK")
}
