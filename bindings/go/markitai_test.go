package markitai

import (
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"
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
