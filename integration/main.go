/* integration/main.go */
package main

import (
	"flag"
	"fmt"
	"os"
	"strconv"
	"strings"

	"canmi.net/vane-mock-tests/pkg/term"
	"canmi.net/vane-mock-tests/tests"
)

func main() {
	startFrom := flag.Int("start", 1, "Start running tests from ID")
	skipStr := flag.String("skip", "", "Skip tests (e.g., '2,4-6')")
	onlyStr := flag.String("only", "", "Run only specific tests (e.g., '1,3')")
	debug := flag.Bool("debug", false, "Enable debug logs and output")
	serial := flag.Bool("serial", false, "Run tests sequentially")

	flag.Parse()

	if *skipStr != "" && *onlyStr != "" {
		term.Error("Cannot use --skip and --only together.")
		os.Exit(1)
	}

	tests.Initialize()
	allTests := tests.Registry

	if len(allTests) == 0 {
		term.Warn("No tests registered.")
		os.Exit(0)
	}

	var testsToRun []tests.TestCase
	skipSet := parseRange(*skipStr)
	onlySet := parseRange(*onlyStr)

	for _, tc := range allTests {
		if tc.ID < *startFrom {
			continue
		}
		if *onlyStr != "" && !onlySet[tc.ID] {
			continue
		}
		if *skipStr != "" && skipSet[tc.ID] {
			continue
		}
		testsToRun = append(testsToRun, tc)
	}

	if len(testsToRun) == 0 {
		term.Warn("No tests matched the filter criteria.")
		os.Exit(0)
	}

	opts := tests.RunOptions{
		Debug:  *debug,
		Serial: *serial,
	}

	tests.RunTests(testsToRun, opts)
}

func parseRange(input string) map[int]bool {
	res := make(map[int]bool)
	if input == "" {
		return res
	}

	parts := strings.Split(input, ",")
	for _, part := range parts {
		part = strings.TrimSpace(part)
		if strings.Contains(part, "-") {
			ranges := strings.Split(part, "-")
			if len(ranges) != 2 {
				term.Warn(fmt.Sprintf("Invalid range format: %s", part))
				continue
			}
			start, _ := strconv.Atoi(ranges[0])
			end, _ := strconv.Atoi(ranges[1])
			if start >= end {
				term.Warn(fmt.Sprintf("Invalid range (start >= end): %s", part))
				continue
			}
			for i := start; i <= end; i++ {
				res[i] = true
			}
		} else {
			val, err := strconv.Atoi(part)
			if err != nil {
				term.Warn(fmt.Sprintf("Invalid number: %s", part))
				continue
			}
			res[val] = true
		}
	}
	return res
}
