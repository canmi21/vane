/* integration/tests/runner.go */
package tests

import (
	"context"
	"fmt"
	"os"
	"sync"
	"time"

	"canmi.net/vane-mock-tests/pkg/env"
	"canmi.net/vane-mock-tests/pkg/term"
)

type RunOptions struct {
	Debug  bool
	Serial bool
}

type TestResult struct {
	Case     TestCase
	Success  bool
	Duration time.Duration
	Error    error
	Logs     string
}

func RunTests(tests []TestCase, opts RunOptions) {
	fmt.Printf("\nrunning %d tests\n", len(tests))

	results := make([]TestResult, len(tests))
	var wg sync.WaitGroup

	maxConcurrency := 8
	if opts.Serial {
		maxConcurrency = 1
	}
	sem := make(chan struct{}, maxConcurrency)

	startTime := time.Now()

	for i, tc := range tests {
		wg.Add(1)
		go func(idx int, testCase TestCase) {
			defer wg.Done()
			sem <- struct{}{}
			defer func() { <-sem }()

			start := time.Now()

			sb, err := env.NewSandbox()
			if err != nil {
				results[idx] = TestResult{Case: testCase, Success: false, Error: fmt.Errorf("sandbox init failed: %w", err)}
				fmt.Printf("test %s ... %sFAILED%s (sandbox error)\n", testCase.Name, term.Red, term.Reset)
				return
			}
			defer sb.Cleanup()

			// FIXED: Use custom context key
			ctx := context.WithValue(context.Background(), env.DebugKey, opts.Debug)
			ctx, cancel := context.WithTimeout(ctx, 30*time.Second)
			defer cancel()

			runErr := testCase.Run(ctx, sb)
			duration := time.Since(start)

			res := TestResult{
				Case:     testCase,
				Duration: duration,
				Success:  runErr == nil,
				Error:    runErr,
			}
			results[idx] = res

			if res.Success {
				fmt.Printf("test %s ... %sok%s\n", testCase.Name, term.Green, term.Reset)
			} else {
				fmt.Printf("test %s ... %sFAILED%s\n", testCase.Name, term.Red, term.Reset)
			}

		}(i, tc)
	}

	wg.Wait()
	printSummary(results, time.Since(startTime))
}

func printSummary(results []TestResult, totalTime time.Duration) {
	passed := 0
	failed := 0

	var failures []TestResult

	for _, r := range results {
		if r.Success {
			passed++
		} else {
			failed++
			failures = append(failures, r)
		}
	}

	if failed > 0 {
		fmt.Println("\nfailures:")
		for i, f := range failures {
			// Clean output: Name followed immediately by the error tree
			term.Printf(term.Red, "%s:\n", f.Case.Name)
			fmt.Printf("%v", f.Error) // FormatFailure already includes indentation

			// Add spacing ONLY if there are more failures
			if i < len(failures)-1 {
				fmt.Print("\n\n")
			} else {
				fmt.Print("\n")
			}
		}
	}

	resultColor := term.Green
	resultText := "ok"
	if failed > 0 {
		resultColor = term.Red
		resultText = "FAILED"
	}

	fmt.Printf("\ntest result: %s%s%s. %d passed; %d failed; 0 ignored; 0 measured; 0 filtered out; finished in %v\n\n",
		resultColor, resultText, term.Reset, passed, failed, totalTime.Round(time.Millisecond))

	if failed > 0 {
		os.Exit(1)
	}
}
