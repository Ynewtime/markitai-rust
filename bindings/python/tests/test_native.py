"""Tests require the built native extension, never a mocked implementation."""

import asyncio
import json
import os
import tempfile
import threading
import unittest
from concurrent.futures import ThreadPoolExecutor
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from unittest.mock import patch

import markitai
from markitai import _native


class NativeTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        old_home = os.environ.get("MARKITAI_HOME")
        os.environ["MARKITAI_HOME"] = str(self.root / "home")

        def restore_home():
            if old_home is None:
                os.environ.pop("MARKITAI_HOME", None)
            else:
                os.environ["MARKITAI_HOME"] = old_home

        self.addCleanup(restore_home)
        self.source = self.root / "文档.md"
        self.source.write_text("# Python\n\n你好 🌍\n", encoding="utf-8")

    def test_typed_results_and_output(self):
        result = markitai.convert(self.source, config={}, llm=False)
        self.assertIsInstance(result, markitai.ConversionOutput)
        self.assertIsInstance(result.usage, markitai.ConversionUsage)
        self.assertIn("你好 🌍", result.markdown)
        self.assertIsNone(result.output_path)
        self.assertEqual(result.assets, [])
        written = markitai.convert(self.source, output_dir=self.root / "out", config={}, llm=False)
        self.assertIsInstance(written.output_path, Path)
        self.assertTrue(written.output_path.is_file())

    def test_async_and_event_loop_guard(self):
        async def exercise():
            with self.assertRaises(RuntimeError):
                markitai.convert(self.source, config={}, llm=False)
            outputs = await asyncio.gather(*(
                markitai.aconvert(self.source, config={}, llm=False) for _ in range(12)
            ))
            self.assertTrue(all("你好 🌍" in item.markdown for item in outputs))

        asyncio.run(exercise())

    def test_repeated_concurrent_native_calls(self):
        with ThreadPoolExecutor(max_workers=4) as pool:
            results = list(pool.map(lambda _: markitai.convert(self.source, config={}, llm=False), range(24)))
        self.assertTrue(all(item.source == str(self.source) for item in results))

    def test_errors_and_config_snapshot(self):
        with self.assertRaises(FileNotFoundError):
            markitai.convert(self.root / "missing.md", config={}, llm=False)
        with self.assertRaises(IsADirectoryError):
            markitai.convert(self.root, config={}, llm=False)
        with self.assertRaises(TypeError):
            markitai.convert(None)
        response = json.loads(_native.convert_json("{"))
        self.assertFalse(response["ok"])
        self.assertTrue(response["error"]["code"])
        config = {"llm": {"enabled": True}}
        markitai.convert(self.source, config=config, llm=False)
        self.assertTrue(config["llm"]["enabled"])

    def test_usage_compatibility(self):
        usage = markitai.ConversionUsage.from_usage_dict(0.12, {
            "model": {"requests": 2, "input_tokens": 5, "output_tokens": 7},
        })
        self.assertEqual((usage.requests, usage.input_tokens, usage.output_tokens), (2, 5, 7))
        markitai.enable_worker_processes()

    def test_config_public_entry_point(self):
        config = markitai.MarkitaiConfig(output={"on_conflict": "overwrite"})
        config.llm.enabled = True
        snapshot = config.model_copy(deep=True)
        snapshot.llm.enabled = False
        self.assertTrue(config.llm.enabled)
        self.assertFalse(snapshot.llm.enabled)
        self.assertIn("你好 🌍", markitai.convert(self.source, config=config, llm=False).markdown)
        self.assertTrue(config.llm.enabled)
        roundtrip = markitai.MarkitaiConfig.model_validate_json(snapshot.model_dump_json())
        self.assertEqual(roundtrip.output.on_conflict, "overwrite")
        with self.assertRaises(ValueError):
            markitai.MarkitaiConfig(output={"on_conflict": "invalid"})

    def test_missing_model_exception(self):
        # Native environment loading also considers cwd/.env. Run this case in
        # the test directory so neither ambient dotenv nor credentials apply.
        previous = Path.cwd()
        try:
            os.chdir(self.root)
            with patch.dict(os.environ, {"MARKITAI_HOME": str(self.root / "home")}, clear=True):
                with self.assertRaises(markitai.NoModelConfiguredError):
                    markitai.convert(self.source, config={}, llm=True)
        finally:
            os.chdir(previous)

    def test_native_releases_gil_during_http(self):
        class Handler(BaseHTTPRequestHandler):
            requests = 0

            def do_GET(self):
                Handler.requests += 1
                body = b"<html><article><h1>Native HTTP</h1><p>The Python server ran.</p></article></html>"
                self.send_response(200)
                self.send_header("Content-Type", "text/html")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

            def log_message(self, *args):
                pass

        server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            url = f"http://127.0.0.1:{server.server_port}/test"
            result = markitai.convert(url, config={}, llm=False)
            self.assertIn("The Python server ran.", result.markdown)
            cached = markitai.convert(url, config={}, llm=False)
            self.assertEqual(cached.markdown, result.markdown)
            self.assertEqual(Handler.requests, 1)
            self.assertFalse(hasattr(cached, "fetch_cache_hit"))
        finally:
            server.shutdown()
            server.server_close()
            thread.join(timeout=5)


if __name__ == "__main__":
    unittest.main()
