"""Small regressions for the documentation check's repository-specific syntax."""
import importlib.util
from pathlib import Path
import tempfile
import unittest
from unittest.mock import MagicMock, patch

spec = importlib.util.spec_from_file_location("doc_links", Path(__file__).parents[1] / "check-doc-links.py")
doc_links = importlib.util.module_from_spec(spec)
spec.loader.exec_module(doc_links)


class DocumentationLinks(unittest.TestCase):
    def test_markdown_images_html_references_and_fences(self):
        text = '''[Guide](guide.md#start) ![image](image.svg)
[Named][ref] [Collapsed][]
[ref]: guide.md "Guide"
[Collapsed]: other.md
<img src="image.webp"><a href="guide.md">Guide</a>
<https://example.com/>
`[example syntax](also-missing.md)`
```toml
example = "[not a link](missing.md)"
```
'''
        links = doc_links.links(text)
        self.assertIn("guide.md#start", links)
        self.assertIn("image.svg", links)
        self.assertIn("other.md", links)
        self.assertIn("image.webp", links)
        self.assertIn("https://example.com/", links)
        self.assertNotIn("missing.md", links)
        self.assertNotIn("also-missing.md", links)
        self.assertIn("missing-reference:undefined", doc_links.links("[Example][undefined]"))

    def test_github_headings_and_duplicates(self):
        self.assertEqual(doc_links.anchors('# Start\n## Start\n## `API` & lifecycle\n## Unicode café\n<a id="custom"></a>'),
                         {"start", "start-1", "api--lifecycle", "unicode-café", "custom"})

    def test_clean_checkout_paths_and_fragments(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            source, target, ignored = (root / p for p in ("README.md", "guide.md", "local.md"))
            source.write_text("# Home\n")
            target.write_text("# Start\n")
            ignored.write_text("local evidence")
            public = {source, target}
            check = lambda link: doc_links.local_error(source, link, public, root)
            self.assertIsNone(check("guide.md#start"))
            self.assertIsNone(check("#home"))
            self.assertIsNone(check("https://example.com/"))
            self.assertEqual(check("guide.md#absent"), "missing heading/anchor")
            self.assertEqual(check("absent.md"), "missing target")
            self.assertIn("ignored", check("local.md"))
            self.assertEqual(check("../outside.md"), "target outside repository")

    def test_runtime_endpoints(self):
        for url in ("http://localhost:8787", "http://127.0.0.1:3000", "http://192.168.1.2/", "http://wled.local/"):
            self.assertTrue(doc_links.runtime_url(url))
        self.assertFalse(doc_links.runtime_url("https://github.com/"))

    def test_external_request_retries_and_reports_failure(self):
        response = MagicMock()
        response.__enter__.return_value.status = 200
        with patch.object(doc_links, "time"):
            with patch.object(doc_links, "urlopen", side_effect=[OSError("temporary"), response]) as request:
                self.assertIsNone(doc_links.external_error("https://example.com/"))
                self.assertEqual(request.call_count, 2)
            with patch.object(doc_links, "urlopen", side_effect=OSError("unavailable")) as request:
                self.assertEqual(doc_links.external_error("https://example.com/"), "unavailable")
                self.assertEqual(request.call_count, 3)
