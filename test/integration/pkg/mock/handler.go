/* integration/pkg/mock/handler.go */
package mock

import (
	"io"
	"net/http"
	"strconv"
)

// SmartEchoHandler is a dynamic handler for testing.
// It behaviors is controlled by request headers:
// - X-Test-Status: Sets the response status code (default 200).
// - Body is always echoed back.
// - Content-Type is preserved.
func SmartEchoHandler(w http.ResponseWriter, r *http.Request) {
	// 1. Determine Status Code
	status := http.StatusOK
	if s := r.Header.Get("X-Test-Status"); s != "" {
		if val, err := strconv.Atoi(s); err == nil {
			status = val
		}
	}

	// 2. Set Headers
	w.Header().Set("X-Upstream-Proto", r.Proto)
	if ct := r.Header.Get("Content-Type"); ct != "" {
		w.Header().Set("Content-Type", ct)
	}

	// 3. Write Status
	w.WriteHeader(status)

	// 4. Echo Body (if not 204/304 which imply no body)
	if status != http.StatusNoContent && status != http.StatusNotModified {
		io.Copy(w, r.Body)
	}
}
