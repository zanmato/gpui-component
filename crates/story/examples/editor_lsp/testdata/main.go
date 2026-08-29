package main

import (
	"errors"
	"fmt"
	"os"
	"strings"
)

// Speaker is implemented by anything that can produce a line of speech.
// Go to Implementation on Speak below jumps to Greeter's method.
type Speaker interface {
	Speak() string
}

// Greeter says hello to whoever it is given.
type Greeter struct {
	Name string
}

// Greet builds the greeting line.
func (g Greeter) Greet() string {
	return fmt.Sprintf("hello %s", g.Name)
}

// Speak implements Speaker.
func (g Greeter) Speak() string {
	return g.Greet()
}

// shout is deliberately mis-formatted; Shift-Alt-F runs gofmt on it.
func shout(s Speaker, exclamations int) string {
level := strings.Repeat("!", exclamations)
	return s.Speak() +   level
}

func main() {
	g := Greeter{Name: "世界🌍 gopher"}
	fmt.Println(g.Greet())
	fmt.Fprintln(os.Stdout, shout(g, 3))
}
