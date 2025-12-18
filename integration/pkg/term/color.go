/* integration/pkg/term/color.go */
package term

import "fmt"

const (
	Reset  = "\033[0m"
	Red    = "\033[31m"
	Green  = "\033[32m"
	Yellow = "\033[33m"
	Blue   = "\033[34m"
	White  = "\033[37m"
	Grey   = "\033[90m"
)

func Printf(color string, format string, a ...interface{}) {
	fmt.Print(color)
	fmt.Printf(format, a...)
	fmt.Print(Reset)
}

func Println(color string, a ...interface{}) {
	fmt.Print(color)
	fmt.Println(a...)
	fmt.Print(Reset)
}

// Pass prints a success message in Green
func Pass(msg string) {
	fmt.Printf("%sok%s %s\n", Green, Reset, msg)
}

// Fail prints a failure message in Red
func Fail(msg string) {
	fmt.Printf("%sFAILED%s %s\n", Red, Reset, msg)
}

// Info prints general info
func Info(msg string) {
	fmt.Println(msg)
}

// Warn prints warnings in Yellow
func Warn(msg string) {
	Println(Yellow, msg)
}

// Error prints errors in Red
func Error(msg string) {
	Println(Red, msg)
}
