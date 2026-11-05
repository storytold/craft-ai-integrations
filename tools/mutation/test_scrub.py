import unittest

from scrub import scrub


class ScrubTests(unittest.TestCase):
    """Keep account names out of verification logs across supported path formats."""

    def test_unix_profiles_keep_the_path_after_the_account(self):
        """Redact Linux and macOS accounts while preserving diagnostic suffixes."""
        for root in ("/home", "/Users"):
            with self.subTest(root=root):
                self.assertEqual(
                    scrub(f'error at "{root}/test-user/project/src/lib.rs:12"'),
                    'error at "%USERPROFILE%/project/src/lib.rs:12"',
                )

    def test_windows_and_msys_profiles_still_match(self):
        """Keep native, escaped, slash-separated and MSYS Windows paths covered."""
        for profile, suffix in (
            (r"C:\Users\test-user", r"\project\src\lib.rs:12"),
            (r"C:\\Users\\test-user", r"\\project\\src\\lib.rs:12"),
            ("C:/Users/test-user", "/project/src/lib.rs:12"),
            ("/c/Users/test-user", "/project/src/lib.rs:12"),
        ):
            with self.subTest(profile=profile):
                self.assertEqual(scrub(profile + suffix), "%USERPROFILE%" + suffix)

    def test_json_escaped_unix_profiles_keep_the_escaped_suffix(self):
        """Redact Unix account names when JSON output escapes every slash."""
        for root in (r"\/home", r"\/Users"):
            with self.subTest(root=root):
                self.assertEqual(
                    scrub(root + r"\/test-user\/project\/src\/lib.rs:12"),
                    r"%USERPROFILE%\/project\/src\/lib.rs:12",
                )

    def test_every_profile_in_a_log_is_scrubbed(self):
        """Redact each account when one log includes several profile paths."""
        self.assertEqual(
            scrub("/home/test-user/a.rs and /Users/another-user/b.rs"),
            "%USERPROFILE%/a.rs and %USERPROFILE%/b.rs",
        )

    def test_embedded_profiles_and_uris_are_scrubbed_conservatively(self):
        """Prioritize account privacy in nested roots, file URIs and web URLs."""
        for path, expected in (
            ("/var/home/test-user/x.rs", "/var%USERPROFILE%/x.rs"),
            ("/mnt/c/Users/test-user/x.rs", "/mnt%USERPROFILE%/x.rs"),
            ("file:///home/test-user/x", "file://%USERPROFILE%/x"),
            ("https://example.com/home/test-user/page", "https://example.com%USERPROFILE%/page"),
        ):
            with self.subTest(path=path):
                self.assertEqual(scrub(path), expected)

    def test_unrelated_paths_and_bare_roots_are_unchanged(self):
        """Leave unrelated paths and roots without an account segment untouched."""
        for path in ("/homepage/test-user/a.rs", "/home", "/home/", "/Users/", "src/lib.rs"):
            with self.subTest(path=path):
                self.assertEqual(scrub(path), path)


if __name__ == "__main__":
    unittest.main()
