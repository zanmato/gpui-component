package main

import "fmt"

// Greeter says hello to whoever it is given.
type Greeter struct {
	Name string
}

// Greet builds the greeting line.
func (g Greeter) Greet() string {
	return fmt.Sprintf("hello %s", g.Name)
}

func main() {
	g := Greeter{Name: "世界🌍 gopher"}
	fmt.Println(g.Greet())
}
