package main

import (
	"bufio"
	"fmt"
	"os"
	"strings"
)

func main() {
	// Print debug info to Stderr
	fmt.Fprintln(os.Stderr, "⚙ Starting execution...")

	// Read all stdin safely
	reader := bufio.NewReader(os.Stdin)
	var inputRaw strings.Builder
	for {
		line, err := reader.ReadString('\n')
		if err != nil {
			if line != "" {
				inputRaw.WriteString(line)
			}
			break
		}
		inputRaw.WriteString(line)
	}

	inputStr := inputRaw.String()
	if len(inputStr) == 0 {
		fmt.Fprintln(os.Stderr, "✗ No input received on Stdin!")
		os.Exit(1)
	}

	inputStr = strings.TrimSuffix(inputStr, "\n")
	fmt.Fprintln(os.Stderr, "⚙ Received Input: "+inputStr)

	// Parse JSON manually for {"auth_token":"..."} structure
	authToken := ""
	tokenKey := "\"auth_token\":\""
	if idx := strings.Index(inputStr, tokenKey); idx != -1 {
		start := idx + len(tokenKey)
		end := strings.Index(inputStr[start:], "\"")
		if end != -1 {
			authToken = inputStr[start : start+end]
		}
	}

	// Business Logic
	var branch, store string
	if authToken == "secret123" {
		fmt.Fprintln(os.Stderr, "✓ Auth success!")
		branch = "success"
		store = `{"user_role":"admin","verified":"true"}`
	} else {
		fmt.Fprintln(os.Stderr, "✗ Auth failed. Token was: "+authToken)
		branch = "failure"
		store = `{"error_reason":"invalid_token"}`
	}

	// Output result to Stdout (compact JSON)
	fmt.Printf("{\"branch\":\"%s\",\"store\":%s}", branch, store)
}
