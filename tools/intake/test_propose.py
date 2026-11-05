"""Black-box tests for the developer proposal command, without any network calls."""

import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("propose.py")


class ProposalCommandTests(unittest.TestCase):
    def setUp(self):
        self.scratch = tempfile.TemporaryDirectory()
        self.addCleanup(self.scratch.cleanup)
        self.path = Path(self.scratch.name) / "candidate.json"

    def call(self, *args):
        return subprocess.run(
            [sys.executable, str(SCRIPT), *map(str, args)],
            text=True,
            capture_output=True,
            check=False,
        )

    def draft(self):
        result = self.call("new", self.path, "--title", "Example mesh workflow", "--kind", "workflow", "--app", "cadcraft")
        self.assertEqual(result.returncode, 0, result.stderr)
        return json.loads(self.path.read_text(encoding="utf-8"))

    def save(self, proposal):
        self.path.write_text(json.dumps(proposal), encoding="utf-8")

    def test_new_does_not_invent_facts_or_overwrite(self):
        proposal = self.draft()
        self.assertEqual(proposal["apps"], ["cadcraft"])
        self.assertIsNone(proposal["integration"]["capability"])
        self.assertEqual(proposal["integration"]["route"], "unknown")
        self.assertEqual(proposal["sources"], [])
        again = self.call("new", self.path, "--title", "Different", "--kind", "model", "--app", "cadcraft")
        self.assertNotEqual(again.returncode, 0)
        self.assertEqual(json.loads(self.path.read_text(encoding="utf-8")), proposal)

    def test_grounded_suggestion_passes_without_implementation_claims(self):
        proposal = self.draft()
        proposal["problem"] = "CADCraft cannot request an AI mesh."
        proposal["sources"] = ["https://example.org/docs"]
        self.save(proposal)
        result = self.call("check", self.path)
        self.assertEqual(result.returncode, 0, result.stderr)
        review = self.call("check", self.path, "--stage", "implementation")
        self.assertNotEqual(review.returncode, 0)
        self.assertIn("integration.capability", review.stderr)

    def test_implementation_requires_a_real_route_and_rejects_prompt_only_mismatch(self):
        proposal = self.draft()
        proposal["problem"] = "Need a mesh from an image."
        proposal["sources"] = ["https://example.org/docs"]
        proposal["integration"].update({
            "provider": "example", "model": "model-v1", "capability": "mesh.generate",
            "inputs": ["source"], "output_media_type": "model/gltf-binary", "route": "declarative",
        })
        proposal["review"].update({
            "auth": "api-key", "billing": "metered", "data_destination": "https://api.example.org",
            "licence_source": "https://example.org/terms",
        })
        self.save(proposal)
        result = self.call("check", self.path, "--stage", "implementation")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("prompt-only", result.stderr)
        proposal["integration"]["route"] = "provider"
        self.save(proposal)
        result = self.call("check", self.path, "--stage", "implementation")
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_secret_fields_and_values_are_rejected_without_echoing_them(self):
        proposal = self.draft()
        proposal["problem"] = "Suggest a route."
        proposal["sources"] = ["https://example.org/docs"]
        proposal["api_key"] = "private-token-do-not-echo"
        self.save(proposal)
        result = self.call("check", self.path)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("secret", result.stderr.lower())
        self.assertNotIn("private-token-do-not-echo", result.stderr)
        del proposal["api_key"]
        proposal["problem"] = "Use Authorization: Bearer private-token-do-not-echo"
        self.save(proposal)
        result = self.call("check", self.path)
        self.assertNotEqual(result.returncode, 0)
        self.assertNotIn("private-token-do-not-echo", result.stderr)

    def test_malformed_types_are_reported_without_a_traceback(self):
        proposal = self.draft()
        proposal["kind"] = []
        proposal["apps"] = ["cadcraft", {}]
        proposal["integration"]["route"] = []
        proposal["review"]["auth"] = []
        self.save(proposal)
        result = self.call("check", self.path)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("kind:", result.stderr)
        self.assertNotIn("Traceback", result.stderr)

    def test_issue_body_requires_a_checked_suggestion_and_never_overwrites(self):
        proposal = self.draft()
        proposal["problem"] = "CADCraft cannot request a mesh."
        proposal["sources"] = ["https://example.org/docs"]
        self.save(proposal)
        issue = Path(self.scratch.name) / "issue.md"
        result = self.call("issue", self.path, "--out", issue)
        self.assertEqual(result.returncode, 0, result.stderr)
        body = issue.read_text(encoding="utf-8")
        self.assertIn("CADCraft cannot request a mesh.", body)
        self.assertIn("https://example.org/docs", body)
        self.assertIn("not a vetted", body.lower())
        self.assertIn("Apps", body)
        again = self.call("issue", self.path, "--out", issue)
        self.assertNotEqual(again.returncode, 0)
        self.assertEqual(issue.read_text(encoding="utf-8"), body)

    def test_issue_preserves_grounded_integration_facts(self):
        proposal = self.draft()
        proposal["problem"] = "Need a mesh."
        proposal["sources"] = ["https://example.org/docs"]
        proposal["integration"].update({"provider": "example", "model": "version-1", "capability": "mesh.generate", "inputs": ["prompt"], "output_media_type": "model/gltf-binary", "route": "provider"})
        proposal["review"].update({"billing": "metered", "auth": "api-key", "data_destination": "https://api.example.org", "licence_source": "https://example.org/terms"})
        self.save(proposal)
        issue = Path(self.scratch.name) / "issue.md"
        result = self.call("issue", self.path, "--out", issue)
        self.assertEqual(result.returncode, 0, result.stderr)
        body = issue.read_text(encoding="utf-8")
        self.assertIn("version-1", body)
        self.assertIn("model/gltf-binary", body)
        self.assertIn("metered", body)

    def test_mesh_result_is_not_cleared_for_declarative_route(self):
        proposal = self.draft()
        proposal["problem"] = "Generate a mesh from a prompt."
        proposal["sources"] = ["https://example.org/docs"]
        proposal["integration"].update({
            "provider": "example", "model": "v1", "capability": "mesh.generate",
            "inputs": ["prompt"], "output_media_type": "model/gltf-binary", "route": "declarative",
        })
        proposal["review"].update({
            "auth": "api-key", "billing": "metered", "data_destination": "https://api.example.org",
            "licence_source": "https://example.org/terms",
        })
        self.save(proposal)
        result = self.call("check", self.path, "--stage", "implementation")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("mesh", result.stderr.lower())

    def test_suggestion_refuses_malformed_source_and_unknown_fields(self):
        proposal = self.draft()
        proposal["problem"] = "Need an API."
        proposal["sources"] = ["http://private.invalid/spec"]
        proposal["integration"]["mystery"] = "ignored"
        self.save(proposal)
        result = self.call("check", self.path)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("integration.mystery", result.stderr)
        self.assertIn("sources[0]", result.stderr)


if __name__ == "__main__":
    unittest.main()
