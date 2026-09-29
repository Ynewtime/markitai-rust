package markitai

import (
	"encoding/json"
	"errors"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"
)

func TestNativeConversion(t *testing.T) {
	dir := t.TempDir()
	t.Setenv("MARKITAI_HOME", filepath.Join(dir, "home"))
	path := filepath.Join(dir, "文档.md")
	if err := os.WriteFile(path, []byte("# Go\n\n你好 🌍\n"), 0600); err != nil {
		t.Fatal(err)
	}
	var group sync.WaitGroup
	for i := 0; i < 24; i++ {
		group.Add(1)
		go func() {
			defer group.Done()
			output, err := Convert(path, &Options{Config: map[string]any{}, LLM: Bool(false)})
			if err != nil {
				t.Error(err)
				return
			}
			if !strings.Contains(output.Markdown, "你好 🌍") || output.OutputPath != nil {
				t.Errorf("unexpected in-memory result: %+v", output)
			}
		}()
	}
	group.Wait()
	output, err := Convert(path, &Options{Config: map[string]any{}, OutputDir: filepath.Join(dir, "out"), LLM: Bool(false)})
	if err != nil {
		t.Fatal(err)
	}
	if output.OutputPath == nil {
		t.Fatal("missing output path")
	}
	if _, err := os.Stat(*output.OutputPath); err != nil {
		t.Fatal(err)
	}
}

func TestErrorsAndMalformedJSON(t *testing.T) {
	t.Setenv("MARKITAI_HOME", t.TempDir())
	_, err := Convert(filepath.Join(t.TempDir(), "missing.md"), &Options{Config: map[string]any{}, LLM: Bool(false)})
	var conversionError *ConversionError
	if !errors.As(err, &conversionError) || conversionError.Code == "" {
		t.Fatalf("expected structured conversion error, got %v", err)
	}
	for _, request := range [][]byte{nil, []byte("{"), {0xff}} {
		response, err := ConvertJSON(request)
		if err != nil {
			t.Fatal(err)
		}
		var envelope struct {
			OK bool `json:"ok"`
		}
		if err := json.Unmarshal(response, &envelope); err != nil || envelope.OK {
			t.Fatalf("invalid error envelope: %s (%v)", response, err)
		}
	}
	if Version() == "" {
		t.Fatal("empty native version")
	}
}

func TestNativeFetchCache(t *testing.T) {
	t.Setenv("MARKITAI_HOME", t.TempDir())
	var requests atomic.Int32
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		requests.Add(1)
		w.Header().Set("Content-Type", "text/html")
		_, _ = w.Write([]byte("<html><article><h1>Native page</h1><p>Go HTTP 世界.</p></article></html>"))
	}))
	defer server.Close()
	options := &Options{Config: map[string]any{}, LLM: Bool(false)}
	first, err := Convert(server.URL, options)
	if err != nil {
		t.Fatal(err)
	}
	second, err := Convert(server.URL, options)
	if err != nil {
		t.Fatal(err)
	}
	if first.Markdown != second.Markdown || !strings.Contains(second.Markdown, "Go HTTP 世界.") {
		t.Fatal("cached Markdown differs from fetched result")
	}
	if requests.Load() != 1 {
		t.Fatalf("expected one HTTP request, got %d", requests.Load())
	}
	options.Config["cache"] = map[string]any{"enabled": false}
	if _, err := Convert(server.URL, options); err != nil {
		t.Fatal(err)
	}
	if requests.Load() != 2 {
		t.Fatalf("disabled cache did not refetch: %d", requests.Load())
	}
}

func TestLegacyErrorUsageIsOptional(t *testing.T) {
	legacy := &ConversionError{Code: "old_code", Message: "old message"}
	if legacy.Error() != "old message" || legacy.Usage != nil {
		t.Fatalf("legacy error constructor changed: %+v", legacy)
	}
	for _, raw := range []string{
		`{"code":"conversion_error","message":"original message"}`,
		`{"code":"conversion_error","message":"original message","usage":null}`,
	} {
		var failure ConversionError
		if err := json.Unmarshal([]byte(raw), &failure); err != nil {
			t.Fatal(err)
		}
		if failure.Error() != "original message" || failure.Code != "conversion_error" || failure.Usage != nil {
			t.Fatalf("older producer contract changed: %+v", failure)
		}
	}
	var recorded ConversionError
	if err := json.Unmarshal([]byte(`{"code":"conversion_error","message":"original message","usage":{"cost_usd":0,"requests":1,"input_tokens":0,"output_tokens":0,"by_model":{"fixture":{"requests":1}}}}`), &recorded); err != nil {
		t.Fatal(err)
	}
	if recorded.Usage == nil || recorded.Usage.Requests != 1 || recorded.Usage.InputTokens != 0 || recorded.Usage.ByModel["fixture"]["requests"] != float64(1) {
		t.Fatalf("known zero-token response was lost: %+v", recorded)
	}
}

func TestPaidNativeFailuresKeepDocumentScopesSeparate(t *testing.T) {
	dir := t.TempDir()
	t.Setenv("MARKITAI_HOME", filepath.Join(dir, "state"))
	t.Setenv("NO_PROXY", "127.0.0.1,localhost")
	files := map[string]string{}
	for _, name := range []string{"TERMALPHA", "TERMBETA", "TERMZERO"} {
		path := filepath.Join(dir, name+".md")
		if err := os.WriteFile(path, []byte("# "+name+"\n\nComplete independent source document "+name+".\n"), 0600); err != nil {
			t.Fatal(err)
		}
		files[name] = path
	}
	var mu sync.Mutex
	requests := map[string]int{}
	var entered atomic.Int32
	gate := make(chan struct{})
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		raw, err := io.ReadAll(io.LimitReader(r.Body, 1024*1024+1))
		if err != nil || len(raw) > 1024*1024 {
			t.Errorf("invalid bounded request: %v", err)
			http.Error(w, "bad fixture request", http.StatusBadRequest)
			return
		}
		name := ""
		for candidate := range files {
			if strings.Contains(string(raw), candidate) {
				name = candidate
				break
			}
		}
		if name == "" {
			t.Error("request lost the independent source marker")
			http.Error(w, "missing fixture marker", http.StatusBadRequest)
			return
		}
		mu.Lock()
		requests[name]++
		mu.Unlock()
		if name != "TERMZERO" {
			if entered.Add(1) == 2 {
				close(gate)
			}
			select {
			case <-gate:
			case <-time.After(10 * time.Second):
				t.Error("both document requests did not reach the server together")
			case <-r.Context().Done():
				return
			}
		}
		tokens := map[string][2]int{"TERMALPHA": {11, 3}, "TERMBETA": {29, 7}, "TERMZERO": {0, 0}}[name]
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(http.StatusUnauthorized)
		if err := json.NewEncoder(w).Encode(map[string]any{
			"error": map[string]any{"message": "PRIVATE RESPONSE SECRET"}, "model": name,
			"usage": map[string]any{"prompt_tokens": tokens[0], "completion_tokens": tokens[1]},
		}); err != nil {
			t.Error(err)
		}
	}))
	defer server.Close()
	config := map[string]any{
		"cache":   map[string]any{"enabled": false, "global_dir": filepath.Join(dir, "cache")},
		"prompts": map[string]any{"dir": filepath.Join(dir, "prompts")},
		"history": map[string]any{"record": false}, "ocr": map[string]any{"enabled": false},
		"image": map[string]any{"alt_enabled": false, "desc_enabled": false},
		"llm": map[string]any{"enabled": true, "on_failure": "fail", "max_requests_per_document": 1,
			"router_settings": map[string]any{"num_retries": 0, "timeout": 15},
			"model_list": []any{map[string]any{"model_name": "fixture", "litellm_params": map[string]any{
				"model": "openai/fixture", "api_key": "synthetic-key", "api_base": server.URL + "/v1",
			}}},
		},
	}
	type answer struct {
		name string
		err  error
	}
	failures := make(chan answer, 2)
	for _, name := range []string{"TERMALPHA", "TERMBETA"} {
		go func(name string) {
			_, err := Convert(files[name], &Options{Config: config})
			failures <- answer{name, err}
		}(name)
	}
	results := []answer{<-failures, <-failures}
	_, zero := Convert(files["TERMZERO"], &Options{Config: config})
	results = append(results, answer{"TERMZERO", zero})
	for _, result := range results {
		var failure *ConversionError
		if !errors.As(result.err, &failure) {
			t.Fatalf("%s: existing ConversionError category lost: %v", result.name, result.err)
		}
		if failure.Code != "conversion_error" || !strings.Contains(failure.Message, "HTTP 401") || strings.Contains(failure.Message, "PRIVATE RESPONSE SECRET") {
			t.Fatalf("%s: unexpected public diagnostic: %+v", result.name, failure)
		}
		tokens := map[string][2]uint64{"TERMALPHA": {11, 3}, "TERMBETA": {29, 7}, "TERMZERO": {0, 0}}[result.name]
		if failure.Usage == nil || failure.Usage.Requests != 1 || failure.Usage.InputTokens != tokens[0] || failure.Usage.OutputTokens != tokens[1] || len(failure.Usage.ByModel) != 1 || failure.Usage.ByModel[result.name] == nil {
			t.Fatalf("%s: missing or mixed accounting: %+v", result.name, failure.Usage)
		}
	}
	mu.Lock()
	defer mu.Unlock()
	if len(requests) != 3 || requests["TERMALPHA"] != 1 || requests["TERMBETA"] != 1 || requests["TERMZERO"] != 1 {
		t.Fatalf("terminal errors retried unexpectedly: %+v", requests)
	}
	_, err := Convert(filepath.Join(dir, "missing.md"), &Options{Config: map[string]any{}, LLM: Bool(false)})
	var early *ConversionError
	if !errors.As(err, &early) || early.Usage != nil {
		t.Fatalf("early failure should not invent accounting: %v", err)
	}
}
